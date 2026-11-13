import { useCenterDwellAi } from './useAutomaticAi';
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { scrollAwayRange, isUserScrollEvent, PROGRAMMATIC_SCROLL_SUPPRESS_MS } from './scrollAwayRead';
import { elementScroll, useVirtualizer, type ReactVirtualizer, type VirtualItem } from '@tanstack/react-virtual';
import { useShallow } from 'zustand/react/shallow';
import {
  useAppStore,
  type AppState,
  LAYOUT_NAMES,
  VIEW_NAMES,
  podcastClickAction,
  autoAiBlockOpen,
  entryNeedsHydration,
  selectAiProductState,
  selectAiCacheStatus,
  selectAiStatusInput,
  selectArticleBody,
  selectVisibleEntries,
  selectFeedConfig,
  resolveFeedLayout,
} from '../store';
import { Icons } from './icons';
import { formatRelativeTime, formatDuration } from '../lib/format';
import { handleArticleLinkClick, openExternalWithFeedback } from '../lib/external';
import { useImageResource } from './useImageResource';
import { canProxyImages, needsImageProxy } from '../lib/imageProxy';
import { galleryMediaKey } from '../lib/galleryMedia';
import { CoverImage } from './CoverImage';
import type { ArticleEntry } from '../types';
import { useEnteringClass } from './useEnteringClass';
import { sentinelMode } from './timelineSentinel';
import { refillDecision } from './timelineRefill';
import { anchorRestoreIndex, clearTopAnchor, ANCHOR_RECORD_THROTTLE_MS, commitTopAnchor, peekReturnAnchor, peekTopAnchor, recordTopAnchor, readerFocusReturnIndex, rearmTopAnchor, stashTopAnchorForReturn } from '../lib/readingPosition';
import { GALLERY_COLUMN_GAP, GALLERY_OVERSCAN, estimateGalleryCardHeight, galleryItemPlacement, galleryTabbableIndex, galleryVisibleIndexes, galleryWindowIndexes, resolveGalleryLaneCount } from './GalleryLayout';
import { HydrateFailure } from './HydrateFailure';
import { anyOverlayOpen } from './shortcutYield';

/* 折叠态外高是不变量，不是估算值：正文区固定 150px，整卡固定 310px。正文是否
   含图、列表行有没有 imageUrl、摘要长短都不参与高度计算。展开或显示译文时
   才退出固定外高，按真实内容增长。虚拟估算必须与这里逐字同源。 */
const SOCIAL_COLLAPSED_BODY_HEIGHT = 150;
const SOCIAL_COLLAPSED_CARD_HEIGHT = 310;
/* .timeline-virtual-item 的固定行间距（padding-bottom: 12px）。虚拟估算必须用
   整行外高，避免首帧总高按 310 算、实测按 322 算而出现滚动高差。 */
const SOCIAL_COLLAPSED_ROW_HEIGHT = SOCIAL_COLLAPSED_CARD_HEIGHT + 12;

type FocusedCard = { id: string; index: number; filterKey: string };

/* 阅读上下文唯一标识（布局|视图|订阅|筛选|排序）。滚动归零、删除焦点恢复与
   focusCardAt 任务都以它判定「是不是同一个列表」；组件内的 filterKey 与焦点
   任务 isCurrent 的实时校验共用同一派生口径。 */
type ReadingContextState = Pick<AppState, 'activeContentLayout' | 'activeViewFilter' | 'activeFeedFilter' | 'timelineFilter' | 'timelineSort'>;
const filterKeyOf = (s: ReadingContextState) => `${s.activeContentLayout}|${s.activeViewFilter}|${s.activeFeedFilter}|${s.timelineFilter}|${s.timelineSort}`;

/** 播放器激活时，卡片上的 Space 让给 App 的全局播放切换；卡内真实交互控件
 *  仍由 App 的 isInteractiveSpaceTarget 让路清单排除。 */
function activePlayerOwnsSpace(e: React.KeyboardEvent): boolean {
  return e.key === ' ' && useAppStore.getState().player.isActive;
}

/** 从卡内任意焦点节点回找卡片身份（不含阅读上下文，由捕获方补 filterKey）；
   画廊的 data-card-index 在 img/占位子节点上。 */
function focusedCardDescriptor(target: Element | null): Omit<FocusedCard, 'filterKey'> | null {
  const card = target?.closest('.article-card, .social-card, .podcast-card, .notif-card, .gallery-card') as HTMLElement | null;
  if (!card?.dataset.id) return null;
  const indexNode = card.matches('[data-card-index]')
    ? card
    : card.querySelector<HTMLElement>('[data-card-index]');
  const index = Number(indexNode?.dataset.cardIndex);
  if (!Number.isInteger(index)) return null;
  return { id: card.dataset.id, index };
}

/* 行虚拟项容器。transform 不进 React 快照（写入者约定见 syncRowGeometryNow），
   但 dataset（data-virtual-start/size）随 commit 落地；若坐标直写只依赖虚拟器
   onChange 与父级 layout effect 兜底，慢调度下会出现「快照已新、transform 仍旧
   值」的跨帧窗口（CI run 5c9f8b5 动作帧：目标已变高、已挂载邻居仍在折叠起点）。
   这里在行项自身 layout effect 中按当次渲染的 vi.start 提交内直写，使任何携带
   新 dataset 的 commit 在同一提交、paint 前把 transform 一并写齐；挂载当帧同样
   由本 effect 与 measureRowElement 落位。Notify/父级 layout effect 仍负责
   commit 间隙（RO 驱动的 resizeItem）与全窗口兜底。
   Note: 见 .agents/notes/implemented/architecture/2026-11-13-界面布局与启动时序.md */
function TimelineRowItem({ vi, item, measureRef, children }: {
  vi: VirtualItem;
  item: ArticleEntry;
  measureRef: (node: HTMLDivElement | null) => void;
  children: ReactNode;
}) {
  const elementRef = useRef<HTMLDivElement | null>(null);
  useLayoutEffect(() => {
    const element = elementRef.current;
    if (element) element.style.transform = `translateY(${vi.start}px)`;
  }, [vi.start]);
  const setNode = useCallback((node: HTMLDivElement | null) => {
    elementRef.current = node;
    measureRef(node);
  }, [measureRef]);
  return (
    <div
      data-index={vi.index}
      data-card-id={item.id}
      data-virtual-start={vi.start}
      data-virtual-size={vi.size}
      ref={setNode}
      className="timeline-virtual-item"
      style={{ position: 'absolute', top: 0, left: 0, width: '100%' }}
    >
      {children}
    </div>
  );
}

/* ============================================================
   Timeline —— 顶栏（标题/筛选/排序/全部已读）+ 五布局渲染器

   交互设计：
   - 列表容器在布局/视图/筛选切换时做一次 160ms 的淡入过渡，
     避免内容瞬间替换造成的视觉跳动（"闪一下"）。
   - 列表切换后滚动位置归零（新列表从顶部阅读）。
     TASK-115 例外：切布局/视图/范围后**切回**且缓存命中时，按 per-filterKey
     锚存档恢复到离开时的位置（返回位置统一规则，见 readingPosition.ts
     头注规则表）；其余切换仍归零。

   展示口径：源名称/分类由 feedId 经解析表派生（不冗余存储）；
   时间为相对时间（由 publishedAt 派生，每分钟自然刷新）。
   ============================================================ */

export function Timeline() {
  const activeContentLayout = useAppStore((s) => s.activeContentLayout);
  const activeViewFilter = useAppStore((s) => s.activeViewFilter);
  const activeFeedFilter = useAppStore((s) => s.activeFeedFilter);
  const timelineFilter = useAppStore((s) => s.timelineFilter);
  const timelineSort = useAppStore((s) => s.timelineSort);
  const categories = useAppStore((s) => s.categories);
  const selectArticle = useAppStore((s) => s.selectArticle);
  const toggleTimelineFilter = useAppStore((s) => s.toggleTimelineFilter);
  const toggleTimelineSort = useAppStore((s) => s.toggleTimelineSort);
  const markCurrentViewAllRead = useAppStore((s) => s.markCurrentViewAllRead);
  const loadMoreArticles = useAppStore((s) => s.loadMoreArticles);
  const articlesLoading = useAppStore((s) => s.articlesLoading);
  const backendReloadsInFlight = useAppStore((s) => s.backendReloadsInFlight);
  const articlesExhausted = useAppStore((s) => s.articlesExhausted);
  const activeArticleId = useAppStore((s) => s.activeArticleId);

  /* 返回新数组的派生 selector 必须包 useShallow */
  const items = useAppStore(useShallow(selectVisibleEntries));

  /* ---------- 列表卡片的 roving tabindex（REQ-008 焦点可达性） ----------
     成组卡片不能各自可 Tab：否则 Tab 会逐个走过几十张可见卡片。
     组内只保留一个可 Tab 进入（tabIndex=0），组内用方向键移动——
     与既有 J/K 键盘流同一语义，不新增第二套导航。
     focusIndex 跟随 activeArticleId（搜索/命令面板/J-K 选中后焦点同步）。 */
  const [focusIndex, setFocusIndex] = useState(0);
  const [liveMessage, setLiveMessage] = useState('');
  const focusedCardRef = useRef<FocusedCard | null>(null);
  const previousItemsRef = useRef(items);
  const lastSyncedSelectionRef = useRef<string | null>(null);
  /* TASK-115②：最后选中的文章 id（关闭信号消费侧的归还目标）。activeArticleId
     跟随 effect 顺带记账——关闭阅读器的时刻它必然持有阅读器正在显示的文章
     （关闭后置 null 的分支不更新，ref 保留关闭前的值）。选型理由见
     readingPosition.ts 头注 X2 节（store 不反向依赖 components 层）。 */
  const lastActiveArticleIdRef = useRef<string | null>(null);
  const focusTaskRef = useRef<{ frame?: number; timer?: ReturnType<typeof setTimeout> }>({});
  const cancelCardFocus = () => {
    const task = focusTaskRef.current;
    if (task.frame !== undefined) cancelAnimationFrame(task.frame);
    if (task.timer !== undefined) clearTimeout(task.timer);
    focusTaskRef.current = {};
  };
  useEffect(() => {
    const onFocus = (event: FocusEvent) => {
      if ((event.target as HTMLElement)?.closest('[role="menu"], [role="dialog"], .modal-overlay')) cancelCardFocus();
    };
    // contextmenu 的捕获阶段先撤销旧任务，不依赖 React effect 的提交次序。
    document.addEventListener('contextmenu', cancelCardFocus, true);
    document.addEventListener('focusin', onFocus);
    return () => {
      cancelCardFocus();
      document.removeEventListener('contextmenu', cancelCardFocus, true);
      document.removeEventListener('focusin', onFocus);
    };
  }, []);

  /* 阅读上下文唯一标识：滚动归零（filterKey effect）、删除焦点恢复与
     focusCardAt 过期任务判定都以它为「同一个列表」的依据，派生口径与
     模块级 filterKeyOf 一致。 */
  const filterKey = filterKeyOf({ activeContentLayout, activeViewFilter, activeFeedFilter, timelineFilter, timelineSort });

  /* 方向键在卡片间移动：目标可能未被虚拟化渲染，先 scrollToIndex 再于下一帧聚焦 */
  /* TASK-115②：定位-聚焦动作收口为 focusCardAt（绝对下标），焦点归还与方向键
     移动共用同一套「setFocusIndex → 程序性滚动抑制 → scrollToIndex(align:auto)
     → 下一帧聚焦」——align:auto 语义正好满足契约「原卡可见不滚、不可见才
     scrollToIndex 定位」，两种入口不再各写一份。 */
  const focusCardAt = (index: number) => {
    cancelCardFocus();
    if (index < 0 || index >= items.length) return;
    const targetId = items[index].id;
    /* 任务里的 rAF/定时器链可能比一次上下文切换活得久：过期任务的原生 focus
       会把已作废的列表滚向旧上下文的位置。isCurrent 按实时 store 校验发起时
       的阅读上下文，切换即视为过期（与恢复 effect 的 filterKey 门控同一依据）。 */
    const taskFilterKey = filterKey;
    const task = focusTaskRef.current;
    const isCurrent = () => {
      const state = useAppStore.getState();
      return focusTaskRef.current === task && filterKeyOf(state) === taskFilterKey
        && !anyOverlayOpen({ ...state, playerActive: state.player.isActive })
        && !document.activeElement?.closest('[role="menu"], [role="dialog"]');
    };
    if (!isCurrent()) return;
    setFocusIndex(index);
    suppressNextScrollEvents();
    /* 画廊走 lanes 虚拟化实例（与行虚拟化同容器同 align:auto 语义：原卡可见
       不滚、不可见才 scrollToIndex 定位），其余布局走行虚拟化实例。 */
    const focusVirtualizer = activeContentLayout === 'image' ? galleryVirtualizer : rowVirtualizer;
    focusVirtualizer.scrollToIndex(index, { align: 'auto' });
    let attempts = 0;
    const focusWhenMounted = () => {
      if (!isCurrent()) return;
      const root = scrollRef.current;
      const el = root?.querySelector<HTMLElement>(`[data-card-index="${index}"]`);
      const cardId = el?.closest<HTMLElement>('[data-ctx="article"]')?.dataset.id;
      if (el && cardId === targetId) {
        /* 虚拟定位可能尚在测量后一帧才完全落定；焦点默认滚动作为第二道可见性保证。 */
        el.focus();
        let visibilityChecks = 0;
        const keepVisible = () => {
          if (!isCurrent() || !el.isConnected || !root) return;
          const rootRect = root.getBoundingClientRect();
          const cardRect = el.getBoundingClientRect();
          if (cardRect.top < rootRect.top || cardRect.bottom > rootRect.bottom) {
            el.scrollIntoView({ block: 'nearest', inline: 'nearest' });
          }
          if (visibilityChecks++ < 4) task.timer = setTimeout(keepVisible, 30);
        };
        keepVisible();
        return;
      }
      if (attempts++ < 3) task.frame = requestAnimationFrame(focusWhenMounted);
    };
    task.frame = requestAnimationFrame(focusWhenMounted);
  };
  const moveCardFocus = (from: number, delta: number) => {
    const next = from + delta;
    if (next < 0 || next >= items.length) return;
    focusCardAt(next);
  };

  /* 列表长度变化（筛选/切换订阅/重新加载）后 focusIndex 可能越界——
     越界会导致没有任何卡片 tabIndex=0，键盘就再也进不了列表。
     这里夹取到一个必然存在的下标，保证列表中**始终有一张卡可 Tab 进入**。 */
  const tabbableIndex = items.length === 0 ? -1 : Math.min(focusIndex, items.length - 1);

  /* J/K、搜索与命令面板选中文章后，DOM 焦点必须落到目标卡；卡片可能尚在
     虚拟化窗口外，focusCardAt 会先 scrollToIndex 再等挂载。只对「选中 id 首次
     变化」生效，后台刷新 items 不抢用户当前焦点。 */
  useEffect(() => {
    if (!activeArticleId) {
      lastSyncedSelectionRef.current = null;
      return;
    }
    lastActiveArticleIdRef.current = activeArticleId;
    if (lastSyncedSelectionRef.current === activeArticleId) return;
    const idx = items.findIndex((a) => a.id === activeArticleId);
    if (idx < 0) return;
    lastSyncedSelectionRef.current = activeArticleId;
    focusCardAt(idx);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeArticleId, items]);

  /* 当前持焦卡被收藏/筛选移除时，React 卸载节点会把焦点甩到 BODY。这里以
     「移除前的卡片身份 + 下标」恢复：优先同下标的后一张，末尾则夹到新的末项；
     用户焦点在筛选控件/侧栏时不抢焦点。恢复仅在产生焦点的同一阅读上下文内
     生效：filterKey 变化的 items 替换是另一套条目，旧下标恢复会与上下文归零
     竞争把列表滚向陈旧深滚位置。
     Note: 根因与运行时证据见 .agents/notes/implemented/architecture/2026-11-13-界面布局与启动时序.md */
  useEffect(() => {
    const previousItems = previousItemsRef.current;
    const focusedCard = focusedCardRef.current;
    if (!focusedCard || focusedCard.filterKey !== filterKey) {
      previousItemsRef.current = items;
      return;
    }
    const currentById = new Map(items.map((item) => [item.id, item]));
    const previousById = new Map(previousItems.map((item) => [item.id, item]));

    const active = document.activeElement as HTMLElement | null;
    const focusLost = !active
      || active === document.body
      || active === document.documentElement
      || !active.isConnected;
    const currentItem = currentById.get(focusedCard.id);
    const previousItem = previousById.get(focusedCard.id);

    if (!currentItem && previousItem && focusLost) {
      const fallbackIndex = Math.min(focusedCard.index, items.length - 1);
      const fallback = fallbackIndex >= 0 ? items[fallbackIndex] : null;
      const liveItem = useAppStore.getState().entries.find((entry) => entry.id === focusedCard.id);
      const action = previousItem.isStarred && liveItem && !liveItem.isStarred
        ? '已取消收藏'
        : '已从当前列表移除';
      setLiveMessage(`${action}「${previousItem.title}」${fallback ? `，焦点移至「${fallback.title}」` : '，列表已为空'}`);
      if (fallback) {
        focusedCardRef.current = { id: fallback.id, index: fallbackIndex, filterKey };
        focusCardAt(fallbackIndex);
      } else {
        focusedCardRef.current = null;
        scrollRef.current?.focus({ preventScroll: true });
      }
    } else if (currentItem && previousItem && currentItem.isStarred !== previousItem.isStarred) {
      setLiveMessage(`${currentItem.isStarred ? '已收藏' : '已取消收藏'}「${currentItem.title}」`);
    }
    previousItemsRef.current = items;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [items]);

  /* 筛选上下文变化 → 滚动归零。不再用 key 重挂载整个列表 DOM（此前每次
     布局/视图/排序切换都强制卸载重建全部卡片 + 重建全部 IntersectionObserver/
     ResizeObserver，是「切换卡顿 + 加载正文闪动」的主因）；改为复用 DOM，
     React 只 diff 列表项，内容切换即时可见。 */
  useEffect(() => {
    /* 程序性归零：先开抑制窗口再滚，随后到达的 scroll 事件不算用户滚动 */
    suppressNextScrollEvents();
    document.getElementById('timelineContentScroll')?.scrollTo({ top: 0 });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filterKey]);

  /* ---------- 滚动出列表视口 → 标已读（markReadOnScrollOut） ---------- */
  const scrollRef = useRef<HTMLDivElement>(null);

  /* ---------- 画廊列模型参数（OPT-017C 有界渲染） ----------
     列数：与旧 CSS 多列断点逐字一致（≤800 → 2，≤1200 → 3，否则 4；单点在
     GalleryLayout.resolveGalleryLaneCount）。窗口跨断点时重建列模型
     （lanes 变化 = 列归属与宽度整体重排，虚拟化内部按 lanes 变更清零重算）。 */
  const [galleryLanes, setGalleryLanes] = useState(() => resolveGalleryLaneCount());
  useEffect(() => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return;
    const narrow = window.matchMedia('(max-width: 800px)');
    const mid = window.matchMedia('(max-width: 1200px)');
    const update = () => setGalleryLanes(resolveGalleryLaneCount());
    narrow.addEventListener?.('change', update);
    mid.addEventListener?.('change', update);
    return () => {
      narrow.removeEventListener?.('change', update);
      mid.removeEventListener?.('change', update);
    };
  }, []);
  /* 容器宽度 → 未挂载卡片的估算高度（只影响窗口定位与滚动总高；挂载后的真实
     高度由 measureElement 修正）。ResizeObserver 观察滚动容器：首帧与宽度
     变化时重估；无 RO（旧环境）时只取首帧宽度。 */
  const [galleryWidth, setGalleryWidth] = useState(0);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const update = () => setGalleryWidth(el.offsetWidth);
    update();
    if (typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  /* 布局/视图/筛选/排序切换时列表淡入一次（REQ-005）。此前 .list-entering 在
     CSS 里声明了却从未被应用，是死代码。 */
  useEnteringClass(
    scrollRef,
    `${activeContentLayout}|${activeViewFilter}|${activeFeedFilter}|${timelineFilter}|${timelineSort}`,
    'list-entering',
  );

  /* 虚拟滚动：只渲染视口 + overscan 缓冲内的条目（约 30 条），滚动时复用 DOM，
     彻底消除「一次性渲染 500 张含正文卡片」的卡顿（成熟 RSS 客户端的共识做法）。
     画廊（image）走第二个实例（lanes 列模型，见下）；两实例共用同一滚动容器，
     同一时刻只有当前布局的那个处于 enabled。 */
  const rowPendingScrollRef = useRef<{ offset: number; adjustments: number; behavior?: ScrollBehavior } | null>(null);
  const rowVirtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => scrollRef.current,
    /* 折叠态是统一固定外高，所有社交行共用同一估算；不再读取摘要/封面特征。
       展开后的真实高度仍由 SocialCard 的 layout effect 显式回写。Note: 见
       .agents/notes/implemented/architecture/2026-11-13-界面布局与启动时序.md */
    estimateSize: () => activeContentLayout === 'social' ? SOCIAL_COLLAPSED_ROW_HEIGHT : 160,
    overscan: 8,
    getItemKey: (index) => items[index]?.id ?? index,
    enabled: activeContentLayout !== 'image',
    /* 与画廊实例（下方）同口径：卡片测量回调不走 flushSync 强制同步提交。
       Note: 依据与取舍见同文件画廊实例注释与 docs/performance.md。 */
    useFlushSync: false,
    scrollToFn: (offset, options, instance) => {
      if (options.adjustments) {
        rowPendingScrollRef.current = { offset, adjustments: options.adjustments, behavior: options.behavior };
      } else elementScroll(offset, options, instance);
    },
    onChange: (instance) => {
      /* 测量驱动的重排（含 measureElement 的 ResizeObserver 路径）只更新虚拟化
         内部测量；transform 与容器高必须在同一次 notify 里直写 DOM，否则慢调度
         下会 paint 出「卡片已变高、邻居仍在旧位置」的一帧。滚动消费仍按契约
         排在坐标发布之后。
         Note: 行坐标单一写入者与采样对齐 — 见 .agents/notes/implemented/architecture/2026-11-13-界面布局与启动时序.md */
      syncRowGeometryNow();
      consumeDeferredScroll(rowPendingScrollRef, instance);
    },
  });
  /* 社交卡展开/收起或显示译文时，外层高度会变化；等 RO 异步回写会让相邻虚拟项
     多保留一帧旧起点。子卡在 layout effect 中同步报告外框高度，这里更新虚拟化
     测量并触发一次 paint 前重渲染。 */
  const syncRowGeometryNow = useCallback(() => {
    const root = activeContentLayout === 'image' ? null : scrollRef.current;
    const wrap = root?.querySelector<HTMLElement>('.timeline-virtual-wrap');
    if (!root || !wrap) return;
    wrap.style.height = `${rowVirtualizer.getTotalSize()}px`;
    for (const virtualItem of rowVirtualizer.getVirtualItems()) {
      const element = root.querySelector<HTMLElement>(
        `.timeline-virtual-item[data-index="${virtualItem.index}"]`,
      );
      if (element) element.style.transform = `translateY(${virtualItem.start}px)`;
    }
  }, [activeContentLayout, rowVirtualizer]);
  const [, setRowLayoutEpoch] = useState(0);
  const syncSocialItemHeight = useCallback((index: number, height: number) => {
    rowVirtualizer.resizeItem(index, height);
    /* 子卡的 layout effect 同步执行到这里时，直接写 transform；不能等父级下一
       次 render——内容 DOM 已经提交，晚一帧就是可见的卡片重叠。 */
    syncRowGeometryNow();
    setRowLayoutEpoch((epoch) => epoch + 1);
  }, [rowVirtualizer, syncRowGeometryNow]);
  /* 后代 layout effect 全部跑完后的兜底回写；滚动/布局切换后的普通 render
     也走这里，保持 React 快照与直接写 DOM 的同值。 */
  useLayoutEffect(() => {
    syncRowGeometryNow();
  });
  /* 行虚拟项挂载/复用时在 ref 回调（commit layout 阶段附着、paint 前）落位。
     虚拟器重新启用或窗口扩张的首个 commit 渲染时可能还没有 scrollRect，JSX
     先渲染出空窗口，卡片要等下一次 rerender 才插入；与画廊
     measureGalleryElement 同模式：measureElement 同步测量后按当次渲染的测量
     写坐标，插入即带定位，不依赖 querySelector/根查找与 effect 时序。
     key 守卫只拒绝不匹配条目的坐标写入（测量/缓存注册仍照常进行）；正常
     enabled、key 匹配的挂载定位由此保证，更新路径由 geom 直写与行项
     layout effect 收敛。
     Note: 见 .agents/notes/implemented/architecture/2026-11-13-界面布局与启动时序.md */
  const measureRowElement = useCallback((element: HTMLDivElement | null) => {
    rowVirtualizer.measureElement(element);
    if (!element) return;
    const measurement = rowVirtualizer.measurementsCache[Number(element.dataset.index)];
    if (measurement && String(measurement.key) === element.dataset.cardId) {
      element.style.transform = `translateY(${measurement.start}px)`;
    }
  }, [rowVirtualizer]);
  /* 画廊有界渲染：lanes 列模型 + 垂直虚拟化。可见范围 = 每列视口覆盖条数 +
     两端 overscan，挂载数与条目总数无关（窗口上界推导、列宽公式与「为什么
     不用 CSS 多列」见 GalleryLayout.ts 头注与 docs/performance.md 画廊节）。
     gap=14 与旧 column-gap / 卡片 margin-bottom 同值：列宽与纵向间距观感不变。 */
  const galleryMeasureAnchorRef = useRef<{ id: string; index: number; lane: number; scrollTop: number } | null>(null);
  const galleryPendingScrollRef = useRef<{ offset: number; adjustments: number; behavior?: ScrollBehavior } | null>(null);
  const galleryWindowRef = useRef('');
  const [, setGalleryWindowEpoch] = useState(0);
  const galleryIndexes = (instance: ReactVirtualizer<HTMLDivElement, Element>, offset = instance.scrollOffset ?? 0) => galleryWindowIndexes(
    instance.measurementsCache,
    offset,
    instance.scrollRect?.height ?? 0,
    galleryLanes,
  );
  const galleryWindowKey = (instance: ReactVirtualizer<HTMLDivElement, Element>, offset = instance.scrollOffset ?? 0) => {
    const indexes = galleryIndexes(instance, offset);
    const visible = galleryVisibleIndexes(indexes.map((index) => instance.measurementsCache[index]), offset, instance.scrollRect?.height ?? 0);
    return `${indexes.join(',')}:${galleryTabbableIndex(visible, tabbableIndex)}`;
  };
  /* 补偿基于产生时刻的 offset；该值与元素真实 scrollTop 脱节（重启用首帧的陈旧
     窗口测量、或 scrollToIndex 与测量同 commit 的暂态）时补偿不可信，丢弃而非
     滚向陈旧位置。正常解码补偿两者一致，逐帧消费不受影响。1.5px 与 virtual-core
     的 approxEqual 同容差。 */
  const consumeDeferredScroll = (
    pendingRef: { current: { offset: number; adjustments: number; behavior?: ScrollBehavior } | null },
    instance: { scrollElement?: { scrollTo?: (options: { top: number; behavior?: ScrollBehavior }) => void } | null },
    readLayoutBeforePublish?: () => void,
  ) => {
    const pending = pendingRef.current;
    if (!pending) return;
    pendingRef.current = null;
    const element = scrollRef.current;
    if (element && Math.abs(pending.offset - element.scrollTop) > 1.5) return;
    /* CI run 42462857980 画廊波：业务几何直写与补偿发布同栈完成，但可观察采样
       里原生 scrollTop 先于坐标/总高变化（+295/+159 各一坏帧）。发布前对真实
       网格读一次布局，让本同步点先收敛布局再发布滚动；不等待帧、不改补偿量与
       锚语义。只挂在画廊补偿消费上——普通滚动、无补偿 onChange 与 row 路径不
       增加同步布局读取。可测试候选，有效性由目标 CI 证明。Note: 见
       .agents/notes/implemented/architecture/2026-11-13-界面布局与启动时序.md */
    readLayoutBeforePublish?.();
    instance.scrollElement?.scrollTo?.({ top: pending.offset + pending.adjustments, behavior: pending.behavior });
  };
  /* virtual-core 的 getScrollOffset 在 .d.ts 标记 private，但运行时是实例方法
     （3.17.8 `this.getScrollOffset = ...`），enabled=false 时调用它会把内部
     offset 置空。这里只取该副作用，返回值弃用。 */
  const forceResetStaleScrollOffset = (instance: object) => {
    (instance as { getScrollOffset?: () => number }).getScrollOffset?.();
  };
  const syncGalleryGeometry = (instance: ReactVirtualizer<HTMLDivElement, Element>) => {
    const height = instance.getTotalSize(); // 同时重建刚 resize 的测量表。
    const container = scrollRef.current?.querySelector<HTMLElement>('.gallery-masonry-grid');
    if (container) container.style.height = `${height}px`;
    for (const element of instance.elementsCache.values()) {
      const measurement = instance.measurementsCache[Number(element.getAttribute('data-index'))];
      if (measurement && String(measurement.key) === (element as HTMLElement).dataset.cardId) {
        (element as HTMLElement).style.transform = `translateY(${measurement.start}px)`;
      }
    }
    // core 先更新逻辑 offset，业务先发布全部已挂节点坐标/容器，再发布 DOM scrollTop。
    // 禁止先滚、等下一 React commit 才补坐标；不改正常用户滚动或 scrollToIndex。
    consumeDeferredScroll(galleryPendingScrollRef, instance, () => {
      const grid = scrollRef.current?.querySelector<HTMLElement>('.gallery-masonry-grid');
      if (grid) void grid.offsetHeight;
    });
  };
  const galleryVirtualizer: ReactVirtualizer<HTMLDivElement, Element> = useVirtualizer({
    count: items.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => estimateGalleryCardHeight(galleryWidth, galleryLanes),
    overscan: GALLERY_OVERSCAN,
    gap: GALLERY_COLUMN_GAP,
    lanes: galleryLanes,
    getItemKey: (index) => items[index]?.id ?? index,
    enabled: activeContentLayout === 'image',
    rangeExtractor: (): number[] => galleryIndexes(galleryVirtualizer),
    scrollToFn: (offset, options, instance) => {
      if (options.adjustments) {
        galleryPendingScrollRef.current = { offset, adjustments: options.adjustments, behavior: options.behavior };
      } else elementScroll(offset, options, instance);
    },
    /* 测量后的主轴位置与总高由虚拟化直接写 DOM，React 只拥有列宽/左缘，
       避免 scrollTop 已补偿、旧 render 快照又写回上一帧 transform。已离开新
       窗口但尚未卸载的节点也同步位置，避免卸载前保留一帧旧坐标。
       Note: 取舍与异步图片实证见 .agents/notes/implemented/architecture/2026-11-13-界面布局与启动时序.md */
    useFlushSync: false,
    directDomUpdates: true,
    onChange: (instance) => {
      syncGalleryGeometry(instance as ReactVirtualizer<HTMLDivElement, Element>);
      const next = galleryWindowKey(instance as ReactVirtualizer<HTMLDivElement, Element>);
      // adapter 只比较全局 range 两端；同端点下逐列集合也可能变化，必须请求提交新集合。
      if (galleryWindowRef.current !== next) {
        galleryWindowRef.current = next;
        setGalleryWindowEpoch((epoch) => epoch + 1);
      }
    },
  });
  galleryVirtualizer.shouldAdjustScrollPositionOnItemSizeChange = (item, delta) => {
    const root = scrollRef.current;
    if (!root || root.scrollTop <= 0 || activeContentLayout !== 'image') return false;
    let anchor = galleryMeasureAnchorRef.current;
    if (!anchor || Math.abs(anchor.scrollTop - root.scrollTop) > 1 || items[anchor.index]?.id !== anchor.id) {
      const rootTop = root.getBoundingClientRect().top;
      const candidates = [...root.querySelectorAll<HTMLElement>('.gallery-virtual-item')]
        .map((element) => ({ element, top: element.getBoundingClientRect().top - rootTop }))
        .filter(({ top }) => top >= 0 && top < root.clientHeight)
        .sort((left, right) => left.top - right.top || Number(left.element.dataset.index) - Number(right.element.dataset.index));
      const element = candidates[0]?.element;
      if (!element?.dataset.cardId) return false;
      anchor = {
        id: element.dataset.cardId,
        index: Number(element.dataset.index),
        lane: Number(element.dataset.galleryLane),
        scrollTop: root.scrollTop,
      };
      galleryMeasureAnchorRef.current = anchor;
    }
    // Note: 多列测量只补偿同一可见锚的同列前驱，不能累加其他列的高度差 — 见 .agents/notes/implemented/architecture/2026-11-13-界面布局与启动时序.md
    const adjust = item.lane === anchor.lane && item.index < anchor.index;
    if (adjust) anchor.scrollTop += delta;
    return adjust;
  };
  useLayoutEffect(() => {
    galleryMeasureAnchorRef.current = null;
    galleryPendingScrollRef.current = null;
    rowPendingScrollRef.current = null;
  }, [filterKey, galleryLanes, galleryWidth]);
  const measureGalleryElement = useCallback((element: HTMLDivElement | null) => {
    galleryVirtualizer.measureElement(element);
    if (!element) return;
    const measurement = galleryVirtualizer.measurementsCache[Number(element.dataset.index)];
    if (measurement) element.style.transform = `translateY(${measurement.start}px)`;
  }, [galleryVirtualizer]);
  const activeLayoutRef = useRef(activeContentLayout);
  useLayoutEffect(() => {
    /* 两个虚拟化实例共用同一滚动容器。主消费点在各自 onChange（与测量同栈，
       解码驱动的补偿不等 React render）；本 effect 是兜底消费 + 切换对齐：
       晚于上方清除 effect，切换 commit 里 ref 阶段按陈旧 offset 产生的补偿
       在消费守卫处被丢弃，不再把容器滚向上一布局的深滚位置。 */
    if (activeContentLayout === 'image') {
      syncGalleryGeometry(galleryVirtualizer);
      /* virtual-core 重启用时会用内部 offset 回写元素 scrollTop（_willUpdate 末尾
         的 _scrollToOffset(getScrollOffset())，无 adjustments、绕过补偿守卫）；
         内部 offset 仅在 enabled=false 的 getScrollOffset 调用里被置空。停用方
         主动调用一次，让对方重启用时从 0 开始，不再跳回上次深滚位置。 */
      forceResetStaleScrollOffset(rowVirtualizer);
    } else {
      consumeDeferredScroll(rowPendingScrollRef, rowVirtualizer);
      forceResetStaleScrollOffset(galleryVirtualizer);
    }
    /* TanStack 重新启用时不回读元素真实 scrollTop（observeElementOffset 没有初始回调），
       内部 offset 可能停留在上次画廊滚动位置；首个真实 scroll 事件之前窗口按陈旧
       offset 计算，会短暂挂载深部卡片并把懒加载判定引向屏外。布局变化时派发
       scroll 让两个实例立即对齐元素真实位置；合成事件不带输入闩，不会被误判
       为用户滚动。
       Note: 见 .agents/notes/implemented/architecture/2026-11-13-界面布局与启动时序.md */
    if (activeLayoutRef.current === activeContentLayout) return;
    activeLayoutRef.current = activeContentLayout;
    scrollRef.current?.dispatchEvent(new Event('scroll'));
  });

  /* markReadOnScrollOut：滚动时，可视区起始 index 递增 → 之间的条目「滚出上方」，
     批量标已读。用 ref 记录上次可视区起始 index，在 onChange 里比较。 */
  const lastStartIndexRef = useRef(0);
  /* P3[F5]（AUDIT-20260919-v2）：切换 布局/视图/订阅范围/筛选/排序 会换掉整个 items
     序列并触发上面的 `scrollTo({top: 0})`。但「归零」是异步生效的：在该 effect
     跑到本 effect 之前，本 effect 仍可能读到**切换前的** range.startIndex
     （如切换到 image 布局时虚拟化被禁用，range 会保留旧值）。
     此时 `start > lastStartIndexRef.current` 成立，循环就会把**新序列里**
     index 0..start 的新条目（用户从未见过的）整段标成已读。
     影响面与「旧 startIndex 大小 / 新序列长度」正相关：列表越长越容易命中，
     窗口在滚动归零生效后即关闭——因此表现为「小概率误标已读」，正是 F5 难以复现的原因。
     处置：两个前提都必须成立才认为条目是「滚出上方」——
     (1) 本次 startIndex 变化确由**用户滚动**引起（而非筛选切换引发的程序性归零）；
     (2) 本次 startIndex 相对基准**递增**。
     修后（自检 fix-2）：「用户滚动」不再由「onScroll 触发了」推断——scrollToIndex
     （J/K 定位 / K 顶部回绕 / 搜索锚定）经 element.scrollTo 同样触发容器 scroll 事件，
     修前会被误判成用户滚动而整段标读。现在 scroll 事件必须同时满足
     「近期有真实输入（wheel/touchmove/pointerdown/翻页键）」且「不在程序性滚动
     抑制窗口内」才算用户滚动，判定收口在 scrollAwayRead.isUserScrollEvent。 */
  const scrollDrivenRef = useRef(false);
  /* 真实输入闩：wheel/touchmove/pointerdown/翻页键置真；程序性滚动发起时清掉。 */
  const userGestureRef = useRef(false);
  /* 程序性滚动抑制窗口的结束时刻（performance.now() 毫秒）：scrollToIndex /
     筛选归零发起前推入，窗口内的 scroll 事件一律不算用户滚动。 */
  const programmaticScrollUntilRef = useRef(0);
  /* 程序性滚动包装：suppressNextScrollEvents() 必须在每次 scrollToIndex /
     scrollTo({top:0}) 之前调用——否则随后到达的 scroll 事件可能带着此前残留的
     输入闩被误判为用户滚动。 */
  const suppressNextScrollEvents = () => {
    programmaticScrollUntilRef.current = performance.now() + PROGRAMMATIC_SCROLL_SUPPRESS_MS;
    userGestureRef.current = false;
  };
  /* TASK-123①（审计 P2-5③）：恢复消费点的卡片内像素偏移补加——
     scrollToIndex(align:'start') 把锚卡片顶对齐视口顶，再按锚记录的 offsetPx
     微调 scrollTop = 精确还原视口（长卡片中部的停留位置不再只能恢复卡片顶；
     动态测量下卡片起点可能微移，属既有估算行高误差量级，如实接受——见
     readingPosition.ts 头注①）。px≤0 不产生额外滚动；补偏发生在程序性滚动
     抑制窗口内，不会被误判为用户滚动。 */
  const applyAnchorOffsetPx = (px: number) => {
    if (px <= 0) return;
    const el = scrollRef.current;
    if (el) el.scrollTop += px;
  };
  /* TASK-123①：顶条卡片内像素偏移的统一测量（scrollTop − 顶条卡片虚拟起点）。
     记录（recordTopAnchor）与尾沿补记（commitTopAnchor）共用同一口径，保证
     锚载荷在两条写入路径下语义一致。按行虚拟化实例取值——画廊不参与顶条锚
     （回落口径，见下方三处 image 提前回落），不调用本函数。 */
  const measureAnchorOffsetPx = (topIndex: number): number => {
    const el = scrollRef.current;
    const vi = rowVirtualizer.getVirtualItems().find((v) => v.index === topIndex);
    if (!el || !vi) return 0;
    return Math.max(0, Math.round(el.scrollTop - vi.start));
  };
  /* TASK-123②（审计 P2-5③）：节流尾沿补记——滚动静默 ANCHOR_RECORD_THROTTLE_MS
     后把「最终停留位置」（含卡片内偏移）经 commitTopAnchor 无条件落锚。审计原话：
     「250ms 节流没有尾沿补记，停滚后立即切换还可能记到较早的位置」——节流记录
     （recordTopAnchor）收敛高频事件不变，尾沿补记把最后一次节流采样与真实停滚
     落点之间的相位差抹平：连续滚动的每个事件都重置定时器，滚动停歇后定时器
     独触发一次，落锚必然是停滚时刻的重测值。回调重读实时状态（store getState +
     虚拟化稳定实例）：节流窗口内列表可能已被刷新替换，落锚必须反映停滚时刻的
     真实视口；filterKey 已变化的场景（上下文切换路径已 stash+clear）不得复活
     被丢弃的锚。 */
  const anchorCommitTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => {
    if (anchorCommitTimerRef.current != null) clearTimeout(anchorCommitTimerRef.current);
  }, []);
  const scheduleAnchorCommit = (scheduledFilterKey: string) => {
    if (anchorCommitTimerRef.current != null) clearTimeout(anchorCommitTimerRef.current);
    anchorCommitTimerRef.current = setTimeout(() => {
      anchorCommitTimerRef.current = null;
      const live = useAppStore.getState();
      const liveFilterKey = `${live.activeContentLayout}|${live.activeViewFilter}|${live.activeFeedFilter}|${live.timelineFilter}|${live.timelineSort}`;
      if (liveFilterKey !== scheduledFilterKey) return; // 上下文已切换：锚已存档/清空，尾沿不复活
      if (live.activeContentLayout === 'image') return; // 画廊不参与顶条锚（回落口径，与记录路径一致）
      const topIndex = rowVirtualizer.range?.startIndex ?? 0;
      const top = selectVisibleEntries(live)[topIndex];
      if (!top) return;
      commitTopAnchor(top.id, liveFilterKey, performance.now(), measureAnchorOffsetPx(topIndex));
    }, ANCHOR_RECORD_THROTTLE_MS);
  };
  /* 筛选上下文变化 → 重置基准并关闭本帧的滚出判定。
     与下面的归零 effect 同依赖，按声明顺序先执行 ⇒ 基准与本帧判定都已就绪，
     不依赖「归零 effect 先跑完」这一时序假设。
     TASK-111②：同时丢弃顶条锚——锚属于「上一个阅读上下文」（filterKey 逐字
     一致才允许回位），用户主动切布局/视图/范围/筛选/排序后旧锚绝不参与新
     上下文的后台刷新回位（双重保险：消费侧还有 filterKey 比对）。
     TASK-115①：丢弃前先按锚自身 filterKey 存档（stashTopAnchorForReturn），
     供切回该上下文时恢复——弃锚语义不变（活锚不进新上下文），只是多了归档。 */
  useLayoutEffect(() => {
    lastStartIndexRef.current = 0;
    scrollDrivenRef.current = false;
    /* TASK-115①：离开上下文先把活锚按其 filterKey 存档（切换返回恢复的数据源），
       再沿用既有弃锚语义——先存后清，顺序不可换。任何 filterKey 变化都是
       「离开」（含切排序/筛选）：存档按锚自身 filterKey 记账，回到该上下文
       才可能命中；切排序的新语境查不到档，裁定「不恢复」自然成立（规则表见
       readingPosition.ts 头注）。 */
    stashTopAnchorForReturn();
    clearTopAnchor();
  }, [filterKey]);
  /* 真实输入监听（挂载一次）：四类输入都能启动「用户滚动」的事实——
     wheel（滚轮/触控板）、touchmove（触屏拖动）、pointerdown（滚动条拖动、
     触摸按下）、翻页键（焦点在内部控件上时浏览器滚动最近的可滚祖先）。 */
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const latch = () => { userGestureRef.current = true; };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'PageUp' || e.key === 'PageDown' || e.key === 'Home' || e.key === 'End') latch();
    };
    el.addEventListener('wheel', latch, { passive: true });
    el.addEventListener('touchmove', latch, { passive: true });
    el.addEventListener('pointerdown', latch);
    el.addEventListener('keydown', onKey);
    return () => {
      el.removeEventListener('wheel', latch);
      el.removeEventListener('touchmove', latch);
      el.removeEventListener('pointerdown', latch);
      el.removeEventListener('keydown', onKey);
    };
  }, []);
  useEffect(() => {
    if (!useAppStore.getState().settings.markReadOnScrollOut) return;
    if (timelineFilter !== 'unread') return;
    const start = rowVirtualizer.range?.startIndex ?? 0;
    const scrollDriven = scrollDrivenRef.current;
    scrollDrivenRef.current = false; // 本次判定消费完毕，等待下一次真实滚动
    const { range, nextLastStartIndex } = scrollAwayRange({
      scrollDriven,
      startIndex: start,
      lastStartIndex: lastStartIndexRef.current,
      itemCount: items.length,
    });
    lastStartIndexRef.current = nextLastStartIndex;
    if (!range) return;
    const exitedIds: string[] = [];
    for (let i = range.from; i < range.to; i++) {
      const it = items[i];
      if (it && !it.isRead) exitedIds.push(it.id);
    }
    if (exitedIds.length > 0) {
      useAppStore.getState().markEntriesReadBulk(exitedIds);
    }
  }, [rowVirtualizer.range?.startIndex, items, timelineFilter]);

  /* 选中后的入窗统一由上方 focusCardAt 完成，按当前布局选择虚拟器并先检查
     浮层所有权。旧独立 effect 无条件调用 rowVirtualizer，会在画廊打开灯箱时
     再次滚动并绕过浮层让路；不得重建第二条 activeArticleId → scroll 链。 */

  /* TASK-111②：后台刷新保位——消费顶条锚。reloadFromBackend 以
     keepReadingPosition 落地（feeds-updated / 手动同步 / 单源刷新路径）时
     store bump positionRestoreNonce，本 effect 消费锚：锚 id 仍在新快照中 →
     程序性滚动回其新索引（新文章插入头部场景，抵消整体替换 + 头部插入造成的
     视觉跳动）；无锚 / filterKey 已变 / 锚 id 不在快照 → 回落现状（不强制顶部）。
     分流语义：
     - 锚与 filterKey 一起记账（readingPosition），消费时逐字比对——用户主动
       切范围/布局/视图/排序后锚已被 clearTopAnchor 丢弃且比对也不通过，
       导航路径的 reload 亦不 bump nonce（三重隔离）；
     - 复用 activeArticleId 定位 effect 的既有程序性滚动抑制机制
       （suppressNextScrollEvents → scrollToIndex，防回位被误判成用户滚动
       而整段标读）；align:'start'——锚即「可见首条」，对齐回视口顶 = 原位还原；
     - activeArticleId 的定位 effect 保持既有不变语义（选中文章仍由它负责），
       本 effect 只管滚动位置，两者互补不冲突；
     - 画廊不参与本机制（回落口径，本卡行为保持）：顶条锚的记账/消费两侧都
       跳过 image 布局——列模型虚拟化不改变画廊的锚参与语义，留待后续任务
       单独评估。 */
  const positionRestoreNonce = useAppStore((s) => s.positionRestoreNonce);
  useEffect(() => {
    if (positionRestoreNonce === 0) return; // 初值非信号
    if (activeContentLayout === 'image') return; // 画廊不参与顶条锚回位（回落口径）
    const idx = anchorRestoreIndex(peekTopAnchor(), filterKey, items);
    if (idx == null) return; // 无锚 / 上下文已切换 / 锚丢失 → 回落
    suppressNextScrollEvents();
    rowVirtualizer.scrollToIndex(idx, { align: 'start' });
    /* TASK-123①：卡片内像素偏移补加（对齐卡片顶后按锚 offsetPx 微调，
       精确还原刷新前的视口——含长卡片中部停留位置）。 */
    applyAnchorOffsetPx(peekTopAnchor()?.offsetPx ?? 0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [positionRestoreNonce]);

  /* TASK-115①：切换返回恢复——消费 switchRestoreNonce（一次性定位）。
     nav 三路径缓存命中恢复 entries 时 bump（与本 effect 消费的 items 同一次
     原子写入，effect 闭包里 items/filterKey 已是恢复后的新语境）。存档锚在
     恢复列表 → scrollToIndex(align:'start') 对齐视口顶 = 回到离开时位置；
     无存档（首次进入）/ 锚丢失 → 归零回落（上方 filterKey 归零 effect 已把
     列表置顶，本 effect 不动作即是「顶部」）。
     与 TASK-111 刷新保位（上一 effect）不叠加：本 effect 只由 switchRestoreNonce
     触发、只消费**存档**锚（peekReturnAnchor），不读写 positionRestoreNonce、
     不消费活锚；刷新保位只由 positionRestoreNonce 触发、只消费活锚——触发源、
     锚来源、消费路径三者全部分离（双向隔离由回归网 t115 断言钉住）。
     声明顺序：必须在本文件更早的 filterKey 归零 effect 之后（同批 passive
     effect 按声明序执行——先归零后恢复，恢复定位是最终落点）。
     恢复成功后 rearmTopAnchor 把活锚重锚到落点（理由见 readingPosition.ts：
     防节流窗口把活锚滞留在恢复前位置，用户随即再离开时存档过期锚）。 */
  const switchRestoreNonce = useAppStore((s) => s.switchRestoreNonce);
  useEffect(() => {
    if (switchRestoreNonce === 0) return; // 初值非信号
    if (activeContentLayout === 'image') return; // 画廊不参与存档锚恢复（回落口径）
    const idx = anchorRestoreIndex(peekReturnAnchor(filterKey), filterKey, items);
    if (idx == null) return; // 无存档 / 锚丢失 → 归零回落（不猜）
    suppressNextScrollEvents();
    rowVirtualizer.scrollToIndex(idx, { align: 'start' });
    /* 【TASK-123 改动理由】rearm 补第 4 参（卡片内偏移随重锚落档——恢复落点含
       intra-item 偏移，活锚即停滚时的真实位置；t115-0 锁定的有序链保护意图
       不变：查档 → 决策 → 程序性滚动抑制 → scrollToIndex → 重锚）。 */
    rearmTopAnchor(items[idx].id, filterKey, performance.now(), peekReturnAnchor(filterKey)?.offsetPx ?? 0);
    /* TASK-123①：卡片内像素偏移补加（对齐卡片顶后按存档锚 offsetPx 微调）。 */
    applyAnchorOffsetPx(peekReturnAnchor(filterKey)?.offsetPx ?? 0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [switchRestoreNonce]);

  /* TASK-115②：阅读器关闭焦点归还——消费 readerCloseNonce。关闭阅读器后把
     焦点归还原选中卡（复用 focusCardAt：原卡可见不滚、不可见 scrollToIndex
     定位后聚焦；滚动不动是既有语义——filterKey 未变不触发归零）。
     - 归还目标 = lastActiveArticleIdRef（activeArticleId 跟随 effect 记账，
       关闭时刻持有阅读器正在显示的文章 id）；
     - 原卡不在当前 items（阅读器开着时列表已切换）→ 归零回落不聚焦——
       焦点归还无对象，绝不猜（readerFocusReturnIndex 返回 null）；
     - 画廊不归还（回落口径，本卡行为保持）：列模型虚拟化后就地即可实现，
       但焦点归还语义扩展到画廊超出本卡边界，留待后续任务评估；
       与 TASK-111 回位、切换返回恢复同一回落口径。 */
  const readerCloseNonce = useAppStore((s) => s.readerCloseNonce);
  useEffect(() => {
    if (readerCloseNonce === 0) return; // 初值非信号
    if (activeContentLayout === 'image') return; // 画廊不参与焦点归还（回落口径）
    const idx = readerFocusReturnIndex(lastActiveArticleIdRef.current, items);
    if (idx == null) return; // 列表已切换 / 无档案 → 归零回落
    focusCardAt(idx);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [readerCloseNonce]);

  /* 滚动到底部附近 → 按需加载下一批文章（分页，避免一次性全量拉取）。 */
  const handleScroll = () => {
    if (activeContentLayout === 'image') {
      // global range 两端可能被不同高度的列钉住；用户滚动仍须重评逐列可见并集。
      const next = galleryWindowKey(galleryVirtualizer, scrollRef.current?.scrollTop ?? 0);
      if (galleryWindowRef.current !== next) {
        galleryWindowRef.current = next;
        setGalleryWindowEpoch((epoch) => epoch + 1);
      }
    }
    /* P3[F5]/fix-2：判定收口在 scrollAwayRead.isUserScrollEvent——只有「近期有
       真实输入且不在程序性滚动抑制窗口内」的 scroll 事件才算用户滚动。
       保留既有结论（||）：一次拖拽会产生多个 scroll 事件，标读 effect 只消费
       一次，后续事件不得把已置位的用户滚动结论冲掉。 */
    scrollDrivenRef.current = scrollDrivenRef.current || isUserScrollEvent({
      gestureSeen: userGestureRef.current,
      programmaticUntil: programmaticScrollUntilRef.current,
      now: performance.now(),
    });
    /* TASK-111②：顶条锚记录（节流收口在 readingPosition.recordTopAnchor）——
       以当前可见首条目 id + filterKey 记账，供后台刷新落地后回位。程序性滚动
       （J/K / 回位）也照记：那是用户此刻的阅读位置。画廊不记录（列模型虚拟化
       与顶条锚的语义扩展不在本卡范围；回位消费侧同样回落，两侧口径一致）。
       TASK-123①：锚载荷带卡片内像素偏移（measureAnchorOffsetPx 单点口径）；
       并调度尾沿补记（scheduleAnchorCommit——停滚定稿最终位置，抹平节流相位差）。 */
    if (activeContentLayout !== 'image') {
      const topIndex = rowVirtualizer.range?.startIndex ?? 0;
      const topItem = items[topIndex];
      if (topItem) recordTopAnchor(topItem.id, filterKey, performance.now(), measureAnchorOffsetPx(topIndex));
      scheduleAnchorCommit(filterKey);
    }
    const el = scrollRef.current;
    if (!el) return;
    // 距底部 600px 内视为"到底"，提前预加载，滚动体验更顺滑
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 600) {
      void loadMoreArticles();
    }
  };

  /* TASK-052 空列表补拉 + TASK-094 不足一屏续拉：判定收口在 timelineRefill.refillDecision
     （纯函数，回归网直接断言）。列表为空、或非空但未撑满视口（scrollHeight<=clientHeight，
     onScroll 永不触发）且未到底时，主动续拉直到撑满或到底。连续自动调用有上限
     （AUTO_REFILL_MAX_CALLS）：后端持续返回整页新数据而可见集合不增长时停止，
     交给哨兵的「加载更多」按钮；可见进展（items 增长）或筛选口径变化即重置计数。
     loadMoreArticles 自带入口守卫（在途/已到底直接返回），挂载期重复调用是安全的。 */
  const autoRefillRef = useRef(0);
  const prevItemCountRef = useRef(0);
  const lastFilterKeyRef = useRef('');
  useEffect(() => {
    if (lastFilterKeyRef.current !== filterKey) {
      lastFilterKeyRef.current = filterKey;
      autoRefillRef.current = 0;
      prevItemCountRef.current = items.length;
    }
    if (items.length > prevItemCountRef.current) autoRefillRef.current = 0; // 有可见进展 → 重新计数
    prevItemCountRef.current = items.length;
    const el = scrollRef.current;
    /* filledViewport 与下方哨兵的「可滚动」同一测量：scrollHeight 比 clientHeight
       多出 1px 以上才算撑满（== 视为不可滚动）。 */
    const filledViewport = !!el && el.scrollHeight - el.clientHeight > 1;
    if (
      refillDecision({
        itemCount: items.length,
        exhausted: articlesExhausted,
        loading: articlesLoading,
        filledViewport,
        autoCalls: autoRefillRef.current,
      }) === 'refill'
    ) {
      autoRefillRef.current += 1;
      void loadMoreArticles();
    }
  }, [filterKey, items.length, articlesExhausted, articlesLoading, loadMoreArticles]);

  /* 哨兵「滚动加载更多」只在容器确实可滚动时出现（REQ-107：不可滚动的容器上
     用户执行不了「滚动」）。不可滚动且未到底时改渲染可点击的「加载更多」按钮。
     测量在 layout effect（paint 前）做：稀疏布局首帧不会闪现「滚动加载更多」。
     items/筛选口径变化、窗口尺寸变化都会改变可滚性，故一并监听。 */
  const [listScrollable, setListScrollable] = useState(true);
  useLayoutEffect(() => {
    const update = () => {
      const el = scrollRef.current;
      if (el) setListScrollable(el.scrollHeight - el.clientHeight > 1);
    };
    update();
    window.addEventListener('resize', update);
    return () => window.removeEventListener('resize', update);
  }, [items, filterKey, activeContentLayout]);

  /* 哨兵形态：判定收口在 timelineSentinel.sentinelMode（纯函数，回归网直接断言） */
  const sentinel = sentinelMode(items.length, articlesExhausted, articlesLoading);

  /* 画廊窗口（lanes 虚拟化实例的可见条目）与 roving tabindex 兜底：
     挂载集 = 可见范围 + overscan；焦点卡在窗口内 → 原样保留，不在（用户用
     鼠标滚到别处后按 Tab）→ 兜底窗口首卡，保证任何滚动位置下都恰有一张卡
     可 Tab 进入列表（判定单点在 GalleryLayout.galleryTabbableIndex）。 */
  const galleryVirtualItems = galleryVirtualizer.getVirtualItems();
  const galleryTabbable = activeContentLayout === 'image'
    ? galleryTabbableIndex(galleryVisibleIndexes(galleryVirtualItems, galleryVirtualizer.scrollOffset ?? 0, galleryVirtualizer.scrollRect?.height ?? 0), tabbableIndex)
    : tabbableIndex;

  /* 标题：布局名 [· 视图筛选] [(分类/源名称)] */
  let base = LAYOUT_NAMES[activeContentLayout] ?? '内容';
  if (activeViewFilter !== 'all') base += ` · ${VIEW_NAMES[activeViewFilter]}`;
  if (activeFeedFilter.startsWith('cat-')) {
    const cat = categories.find((c) => c.id === activeFeedFilter);
    if (cat) base += ` (${cat.name})`;
  } else if (activeFeedFilter !== 'all') {
    for (const c of categories) {
      const f = c.feeds.find((x) => x.id === activeFeedFilter);
      if (f) { base += ` (${f.name})`; break; }
    }
  }
  const totalFeedCount = categories.reduce((count, cat) => count + cat.feeds.length, 0);
  const selectedCategory = activeFeedFilter.startsWith('cat-')
    ? categories.find((cat) => cat.id === activeFeedFilter)
    : null;
  const selectedFeedBinding = activeFeedFilter !== 'all' && !activeFeedFilter.startsWith('cat-')
    ? categories
      .flatMap((cat) => cat.feeds.map((feed) => ({ cat, feed })))
      .find(({ feed }) => feed.id === activeFeedFilter)
    : null;
  const layoutHasFeeds = categories.some((cat) =>
    cat.feeds.some((feed) => resolveFeedLayout(feed, cat.layout) === activeContentLayout));
  const emptyCopy = (() => {
    if (totalFeedCount === 0) return '还没有订阅源，先添加一个';
    if (selectedCategory) {
      if (selectedCategory.feeds.length === 0) return '此分类还没有订阅源';
      if (!selectedCategory.feeds.some((feed) =>
        resolveFeedLayout(feed, selectedCategory.layout) === activeContentLayout)) {
        return '此分类没有绑定到当前布局的订阅源';
      }
    }
    if (activeFeedFilter !== 'all' && !selectedCategory) {
      if (!selectedFeedBinding) return '该订阅源已不存在';
      if (resolveFeedLayout(selectedFeedBinding.feed, selectedFeedBinding.cat.layout) !== activeContentLayout) {
        return '该订阅源未绑定到当前布局';
      }
    }
    if (!layoutHasFeeds) return '当前布局还没有绑定订阅源';
    if (activeViewFilter === 'starred') return '暂无收藏内容';
    if (activeViewFilter === 'today') return '今天暂无新内容';
    if (timelineFilter === 'unread') return '没有未读文章';
    return '没有匹配的文章';
  })();

  return (
    <section className="timeline-col">
      <div className="timeline-control-bar">
        <div className="control-bar-main-row">
          <h3 className="view-title-text">{base}</h3>
          <div className="filter-sort-group">
            {activeViewFilter === 'all' && (
              <button className="toggle-action-btn" onClick={toggleTimelineFilter} title={timelineFilter === 'all' ? '显示全部' : '仅显示未读'}>
                <Icons.unreadDot />
                <span>{timelineFilter === 'all' ? '全部' : '未读'}</span>
              </button>
            )}
            <button className="toggle-action-btn" onClick={toggleTimelineSort} title={timelineSort === 'newest' ? '按最新排序' : '按最早排序'}>
              <Icons.sort />
              <span>{timelineSort === 'newest' ? '最新' : '最早'}</span>
            </button>
            <button className="toggle-action-btn" onClick={markCurrentViewAllRead} title="将当前列表全部标为已读">
              <Icons.check />
              <span>全部已读</span>
            </button>
          </div>
        </div>
      </div>

      <div
        className="timeline-scroll-body"
        style={activeContentLayout === 'image' ? { overflowAnchor: 'none' } : undefined}
        id="timelineContentScroll"
        ref={scrollRef}
        onScroll={handleScroll}
        tabIndex={-1}
        onFocusCapture={(e) => {
          const focusedCard = focusedCardDescriptor(e.target as Element);
          if (!focusedCard) return;
          focusedCardRef.current = { ...focusedCard, filterKey };
          setFocusIndex(focusedCard.index);
        }}
      >
        {items.length === 0 && (
          /* 空态/加载态不混同（审计卡2-4）：装载在途且列表为空时不出
             空态文案——数据未落地就宣判「没有内容」是假空态；改渲染
             加载形态，数据落地后按真值恢复（空且已到底 → 文案；有数据 → 列表）。
             此时哨兵同为 loading 形态，这里就不再重复渲染哨兵 spinner。 */
          /* 启动抢占期（bootstrapFromBackend → reloadFromBackend，及筛选视图
             的 reloadFilteredEntries）articlesLoading 尚未置真而 entries 仍是
             旧快照/空数组，只看 articlesLoading 会闪空态假文案；
             故并入后端 reload 在途计数。Note: 计数含兄弟窗口在途 reload，
             不能改用 isBackendReloadInFlight 之类的「有没有」判据 —
             见 .agents/notes/implemented/architecture/2026-11-13-界面布局与启动时序.md */
          articlesLoading || backendReloadsInFlight > 0 ? (
            <div className="timeline-empty-state" role="status" aria-label="加载中">
              <span className="load-more-spinner" aria-hidden="true" />
              <span>加载中…</span>
            </div>
          ) : (
            <div className="timeline-empty-state">
              {emptyCopy}
            </div>
          )
        )}

        {/* 画廊（瀑布流多列）：lanes 列模型 + 垂直虚拟化——只渲染窗口内卡片
            （每列可见条数 × 列数 + overscan，上界与推导见 GalleryLayout.ts /
            docs/performance.md 画廊节），滚动复用同一容器。列宽/列距/图片自然
            比例与旧 CSS 多列一致；列分配由贪心最短列（lanes）决定。 */}
        {activeContentLayout === 'image' && (
          <div
            className="gallery-masonry-grid"
            ref={galleryVirtualizer.containerRef}
            style={{ position: 'relative', width: '100%' }}
          >
            {galleryVirtualItems.map((vi) => {
              const img = items[vi.index];
              if (!img) return null;
              const placement = galleryItemPlacement(vi.lane, galleryLanes);
              return (
                <div
                  key={vi.key}
                  data-index={vi.index}
                  data-card-id={img.id}
                  data-gallery-lane={vi.lane}
                  ref={measureGalleryElement}
                  className="gallery-virtual-item"
                  style={{ position: 'absolute', top: 0, left: placement.left, width: placement.width }}
                >
                  <GalleryCard item={img} cardIndex={vi.index} tabbable={vi.index === galleryTabbable} lanes={galleryLanes} onMoveFocus={moveCardFocus} />
                </div>
              );
            })}
          </div>
        )}

        {/* 其余 4 种单列布局：虚拟滚动，只渲染视口 + overscan 内的条目 */}
        {activeContentLayout !== 'image' && items.length > 0 && (
          <div
            className={`timeline-virtual-wrap ${activeContentLayout !== 'article' ? 'virtual-narrow' : ''}`}
            style={{ height: rowVirtualizer.getTotalSize(), position: 'relative', width: '100%' }}
          >
            {rowVirtualizer.getVirtualItems().map((vi) => {
              const item = items[vi.index];
              if (!item) return null;
              return (
                <TimelineRowItem
                  key={vi.key}
                  vi={vi}
                  item={item}
                  measureRef={measureRowElement}
                >
                  {activeContentLayout === 'article' && <ArticleCard art={item} onSelect={selectArticle} cardIndex={vi.index} tabbable={vi.index === tabbableIndex} onMoveFocus={moveCardFocus} />}
                  {/* TASK-114 X2：Social/Notif 卡补 onSelect（Enter/Space=选中，与 ArticleCard 同语义） */}
                  {activeContentLayout === 'social' && <SocialCard item={item} cardIndex={vi.index} tabbable={vi.index === tabbableIndex} onSelect={selectArticle} onMoveFocus={moveCardFocus} onLayoutHeight={syncSocialItemHeight} />}
                  {activeContentLayout === 'podcast' && <PodcastCard item={item} cardIndex={vi.index} tabbable={vi.index === tabbableIndex} onMoveFocus={moveCardFocus} />}
                  {activeContentLayout === 'notification' && <NotifCard item={item} cardIndex={vi.index} tabbable={vi.index === tabbableIndex} onSelect={selectArticle} onMoveFocus={moveCardFocus} />}
                </TimelineRowItem>
              );
            })}
          </div>
        )}

        {/* 分页加载指示 / 滚动哨兵：加载中显示动画；到底显示「已到底」；否则占位等待滚动。
            TASK-052：此前整块被 `items.length > 0` 挡住——列表为空时哨兵不渲染，
            滚动事件无从触发，「该范围的老文章永远够不到」。列表为空但**批次已满**
            （articlesExhausted=false）时同样渲染：空列表 + 未到底 = 还有数据待取。
            真正到底（空且已到底）时不渲染，避免「没有更多了」与空态文案重复。
            TASK-094（REQ-107）：容器不可滚动时「滚动加载更多」不可执行，改为可点击的
            「加载更多」按钮（原生 button：键盘可聚焦，Enter/空格原生触发）。 */}
        {/* 空列表且装载在途时，加载形态已由空态区渲染（见上），这里让位避免同屏双 spinner。 */}
        {sentinel !== 'hidden' && !(items.length === 0 && (articlesLoading || backendReloadsInFlight > 0)) && (
          <div className="timeline-load-more">
            {sentinel === 'loading' ? (
              <span className="load-more-spinner" aria-label="加载中" />
            ) : sentinel === 'end' ? (
              <span className="load-more-end">没有更多了</span>
            ) : !listScrollable ? (
              <button
                type="button"
                className="toggle-action-btn load-more-btn"
                onClick={() => void loadMoreArticles()}
              >
                加载更多
              </button>
            ) : (
              <span className="load-more-idle">滚动加载更多</span>
            )}
          </div>
        )}
      </div>
      <div className="sr-only" role="status" aria-live="polite" aria-atomic="true" data-live-region="timeline">
        {liveMessage}
      </div>
    </section>
  );
}

/* ---------- 正文懒加载水合 ---------- */

/** 虚拟滚动下，卡片只在进入视口（+overscan 缓冲）时才挂载，挂载即水合正文。
    不再需要 IntersectionObserver 判断「是否进入视口」——虚拟化本身已保证
    挂载的卡片就在视口附近。批量队列（store 的 enqueueHydration）会把同一帧
    内挂载的几十张卡片合并成一次 IPC，避免逐篇洪峰。
    TASK-103（REQ-001）：effect 不能只依赖 [id]——虚拟列表按文章 id 保持卡片
    身份，同 id 不重挂载；若挂载期间水合前提被快照替换重置（reload / 缓存恢复
    后该卡片仍无正文且无终态），旧实现不再触发任何请求，卡片永挂「加载正文…」
    而实际无请求在途（审计探针复现的死区）。修法：按 id 订阅
    entryNeedsHydration 的布尔值（无正文 && 未水合 && 无终态 && 无失败态），
    条件重新成立时翻转触发重新入队；在途重复入队由 hydrateArticleContent 的
    在途去重兜底（不会产生第二次 IPC）。选定该方案而非「reload 完成后统一重
    入队」：后者每次后台刷新都把整页（约 500 条）拉正文，正是懒水合设计刻意
    避免的「列表背正文」洪峰；按卡片观察只覆盖真正挂载着的约 30 张。 */
function useLazyHydrate(id: string): React.RefObject<HTMLDivElement | null> {
  const ref = useRef<HTMLDivElement | null>(null);
  const needsHydration = useAppStore((s) => entryNeedsHydration(s, id));
  useEffect(() => {
    if (!needsHydration) return;
    /* 挂载/条件重新成立即水合（幂等：已有正文或终态则短路） */
    useAppStore.getState().ensureArticleContent(id);
  }, [id, needsHydration]);
  return ref;
}

/* ---------- 文章卡片 ---------- */

const ArticleCard = memo(function ArticleCard({ art, onSelect, cardIndex, tabbable, onMoveFocus }: {
  art: ArticleEntry;
  onSelect: (id: string) => void;
  cardIndex: number;
  tabbable: boolean;
  onMoveFocus: (from: number, delta: number) => void;
}) {
  const activeArticleId = useAppStore((s) => s.activeArticleId);
  const feedName = useAppStore((s) => (s.sourceIndex.get(art.feedId) ?? s.feedIndex.get(art.feedId))?.feed.name ?? '');
  const selected = activeArticleId === art.id;

  return (
    <div
      className={`article-card ${art.isRead ? 'read' : ''} ${selected ? 'active-selected' : ''}`}
      onClick={() => onSelect(art.id)}
      role="button"
      data-card-index={cardIndex}
      tabIndex={tabbable ? 0 : -1}
      aria-current={selected ? 'true' : undefined}
      onKeyDown={(e) => {
        if (activePlayerOwnsSpace(e)) return;
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onSelect(art.id);
          return;
        }
        /* 方向键在卡片间移动（roving）：按当前卡片的视觉位置决定上下游 */
        if (e.key === 'ArrowDown' || e.key === 'ArrowRight') { e.preventDefault(); onMoveFocus(cardIndex, 1); }
        else if (e.key === 'ArrowUp' || e.key === 'ArrowLeft') { e.preventDefault(); onMoveFocus(cardIndex, -1); }
      }}
      data-ctx="article"
      data-id={art.id}
    >
      <div className="card-meta-top">
        <span className="source-tag">{feedName}</span>
        <span>·</span>
        <span>{formatRelativeTime(art.publishedAt)}</span>
        {art.tags.map((t) => (
          <span key={t} className="card-tag-badge">{t}</span>
        ))}
      </div>
      <div className="card-main-content">
        <div className="card-text-col">
          {/* TASK-100 U5：line-clamp 截断文本补 title（=未截断全文） */}
          <h4 className="card-title" title={art.title}>{art.title}</h4>
          <p className="card-snippet" title={art.snippet}>{art.snippet}</p>
        </div>
        {/* 封面：共享 CoverImage（按 imageProxy 判定代理/直连，失败出占位并幂等上报；无 cover 不渲染） */}
        <CoverImage src={art.cover} articleId={art.id} pageUrl={art.url} className="card-cover-thumb" alt="cover" loading="lazy" />
      </div>
      <div className="card-footer">
        <span>{art.author}</span>
        {/* TASK-100 U3：收藏状态视觉统一 Icons.starFilled（此前页脚为文字加星号字符形态，
            与 SocialCard/GalleryCard/Reader 三套视觉并存） */}
        {art.isStarred && (
          <span className="card-starred-flag" title="已收藏"><Icons.starFilled /></span>
        )}
      </div>
    </div>
  );
});

/* ---------- 社交卡片 ---------- */

const SocialCard = memo(function SocialCard({ item, onSelect, cardIndex, tabbable, onMoveFocus, onLayoutHeight }: {
  item: ArticleEntry;
  onSelect: (id: string) => void;
  cardIndex: number;
  tabbable: boolean;
  onMoveFocus: (from: number, delta: number) => void;
  onLayoutHeight: (index: number, height: number) => void;
}) {
  const activeArticleId = useAppStore((s) => s.activeArticleId);
  const toggleEntryFlag = useAppStore((s) => s.toggleEntryFlag);
  const showToast = useAppStore((s) => s.showToast);
  const openLightbox = useAppStore((s) => s.openLightbox);
  const binding = useAppStore((s) => s.sourceIndex.get(item.feedId) ?? s.feedIndex.get(item.feedId));
  const feedConfig = useAppStore(useShallow((s) => selectFeedConfig(s, item.feedId)));
  /* TASK-122：正文/AI 读取单点——selectArticleBody（真值源 bodyById）。
     错误态/空正文终态/加载中都从记录 state 派生（原 hydrationErrors/hydratedIds
     两个按 id 平行订阅随之删除）。
     【为何必须包 useShallow】selectArticleBody 每次调用返回**新对象**（体见
     selectors.ts 的 bodyViewFrom），zustand v5 不做 selector 快照缓存 →
     useSyncExternalStore 每轮拿到新引用，React 报「getSnapshot should be cached」
     并无限重渲染（React #185）。浅比较命中后引用稳定，仅记录真变才重渲染。 */
  const body = useAppStore(useShallow((s) => selectArticleBody(s, item.id)));
  /* 卡片级翻译状态（按 id 订阅，生成中指示） */
  const translatingCard = useAppStore((s) => s.translatingIds[item.id]);
  /* fix-5：卡片级翻译失败信息（内联错误行 + 重试依据，此前只有 toast 一闪而过） */
  const translateError = useAppStore((s) => s.translateErrors[item.id] || '');
  /* TASK-065 N11：译文当前是否为未消毒流式产物（决定纯文本/HTML 渲染路径） */
  const rawTranslated = useAppStore((s) => s.rawTranslatedIds[item.id]);
  /* OPT-013 B3：译文五态（生成中/部分完成/失败/完整/已过期——切片单点判定）。
     partial 的分段进度同源取 aiStatusById（校验状态缓存）。 */
  const translationState = useAppStore((s) => selectAiProductState(s, item.id, 'translation'));
  const translationSegments = useAppStore((s) => selectAiCacheStatus(s, item.id)?.translationSegments ?? null);
  const aiStatusInput = useAppStore(useShallow((s) => selectAiStatusInput(s, item.id)));
  /* 社交卡片正文直接渲染 body.content（真值源 bodyById）：进入视口附近才懒加载
     水合（避免几百张卡片同时 getArticle 卡顿） */
  const hydrateRef = useLazyHydrate(item.id);
  const cardRef = useRef<HTMLDivElement>(null);
  useCenterDwellAi(item.id, cardRef);
  const setCardNode = useCallback((node: HTMLDivElement | null) => {
    hydrateRef.current = node;
    cardRef.current = node;
  }, [hydrateRef]);
  /* 派生值提取为局部变量：JSX 表达式内不放可选链（oxc 解析限制，且更易读） */
  const feedName = binding ? binding.feed.name : '';
  const selected = activeArticleId === item.id;
  /* 三态：null=跟随 feed 配置，true=手动展开，false=手动收起 */
  const [transOverride, setTransOverride] = useState<boolean | null>(null);
  /* fix-8：跟随 auto 配置展开的前提加「有产物/在途/出错」——auto 开但尚无译文时
     按收起处理，不再渲染空壳译文块（自动生成仅在中心连续停顿后触发，
     挂载/焦点本身不触发付费请求）；手动点「翻译」仍会就地触发生成。 */
  const showTranslate = transOverride ?? (translationState === 'partial' || autoAiBlockOpen(
    feedConfig.autoTranslate,
    !!body.translatedContent,
    !!translatingCard,
    !!translateError,
  ));
  /* 折叠态正文区固定 150px：无论正文是否含图、行 imageUrl 是否存在、摘要多短，
     占位与水合后的正文外高都相同。短文和空正文保留同一区域，测量只决定展开
     按钮是否出现，不再参与高度决策。
     Note: 折叠外高是不变量 — 见 .agents/notes/implemented/architecture/2026-11-13-界面布局与启动时序.md */
  const textRef = useRef<HTMLDivElement>(null);
  const innerRef = useRef<HTMLDivElement>(null);
  const [measurement, setMeasurement] = useState<{ content: string; long: boolean } | null>(null);
  const [expanded, setExpanded] = useState(false);
  /* 测量结果与正文逐字绑定：content 一变，旧结果立即失效 → 本次 render 先折叠，
     随后 layout effect 在 paint 前写入新结果。 */
  const currentMeasurement = measurement?.content === body.content ? measurement : null;
  /* 内联 {__html} 每次 render 都是新对象，React 会据此重设 innerHTML，连带重建
     正文里的 <video>。展开/收起等无关 render 一旦重设，视频就重新加载并把
     自然高度打回零；按正文内容锁住对象引用，媒体节点才能跨 render 存活。
     Note: 见 .agents/notes/implemented/architecture/2026-11-13-界面布局与启动时序.md */
  const contentHtml = useMemo(() => ({ __html: body.content }), [body.content]);
  const translatedHtml = useMemo(
    () => ({ __html: body.translatedContent }),
    [body.translatedContent],
  );
  const hydratingBody = !body.content && (body.state === 'unrequested' || body.state === 'loading');
  const fixedCollapsedHeight = !expanded && !showTranslate;
  useLayoutEffect(() => {
    const el = innerRef.current;
    if (!el) return;
    const content = body.content;
    const measure = () => {
      const long = el.scrollHeight > 150;
      setMeasurement((prev) => (
        prev?.content === content && prev.long === long ? prev : { content, long }
      ));
    };
    let measureFrame = 0;
    const scheduleMeasure = () => {
      if (measureFrame) return;
      measureFrame = requestAnimationFrame(() => {
        measureFrame = 0;
        measure();
      });
    };
    /* 观察自然高度内层，而不是固定 150px 外框：图片/视频在 effect 后加载时，
       内层尺寸才会变化；固定外框的 RO 不会触发。 */
    measure();
    const mediaEventNames = ['load', 'error', 'loadedmetadata', 'loadeddata', 'durationchange'];
    const mediaNodes = Array.from(el.querySelectorAll<HTMLImageElement | HTMLVideoElement>('img, video'));
    for (const node of mediaNodes) {
      for (const eventName of mediaEventNames) node.addEventListener(eventName, scheduleMeasure);
    }
    for (const eventName of mediaEventNames) el.addEventListener(eventName, scheduleMeasure, true);
    const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(scheduleMeasure);
    ro?.observe(el);
    return () => {
      if (measureFrame) cancelAnimationFrame(measureFrame);
      ro?.disconnect();
      for (const node of mediaNodes) {
        for (const eventName of mediaEventNames) node.removeEventListener(eventName, scheduleMeasure);
      }
      for (const eventName of mediaEventNames) el.removeEventListener(eventName, scheduleMeasure, true);
    };
  }, [body.content]);

  /* 外层虚拟项高度变化时先同步虚拟化测量，再让父级在本次 paint 前重取位置；
     否则 18 段长文/图片正文落地会晚一帧把后续卡片推到位。 */
  const lastReportedLayoutRef = useRef({ index: -1, height: 0 });
  useLayoutEffect(() => {
    const outer = cardRef.current?.parentElement;
    if (!outer) return;
    const report = () => {
      if (!outer.isConnected) return;
      const height = outer.offsetHeight;
      if (height === lastReportedLayoutRef.current.height
        && cardIndex === lastReportedLayoutRef.current.index) return;
      lastReportedLayoutRef.current = { index: cardIndex, height };
      onLayoutHeight(cardIndex, height);
    };
    report();
    if (typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver(report);
    ro.observe(outer);
    return () => ro.disconnect();
  }, [body.content, body.translatedContent, expanded, showTranslate, cardIndex, onLayoutHeight]);

  /* 仅对虚拟窗口已挂载、正文已水合的卡片取只读状态；没有整篇产物时也能发现
     重启前保留的片段。相同输入只查一次，不在卡片挂载时发起付费生成。 */
  useEffect(() => {
    if (body.state === 'unrequested' || body.state === 'loading') return;
    useAppStore.getState().ensureAiStatus(item.id);
  }, [body.state, aiStatusInput, item.id]);

  return (
    /* TASK-100（UI P2-2 轻修）：社交/通知卡补 article 语义角色 + tabIndex，融入卡片级
       roving tabindex 体系（对照 ArticleCard/PodcastCard）。TASK-114 X2：补
       Enter/Space=选中（与 ArticleCard 同语义 onSelect）。role 维持 article 而非
       改 button：卡内嵌套着动作按钮/链接/重试控件，role=button 会按 ARIA 规则把
       交互后代从可达性树掩蔽掉，且「订阅流中的文章」语义本就是 article——对齐
       ArticleCard 的是**可交互行为**（Enter/Space/tabIndex/roving），不是容器角色。
       Enter/Space 只在焦点落卡片本体（e.target===e.currentTarget）时生效：嵌套
       按钮/链接的键盘激活语义（原生 click 合成）不得被卡片级选中劫持。 */
    <div
      ref={setCardNode}
      className={`social-card ${fixedCollapsedHeight ? 'social-card-collapsed' : 'social-card-expanded'} ${item.isRead ? 'read' : ''} ${selected ? 'active-selected' : ''}`}
      style={fixedCollapsedHeight ? { height: `${SOCIAL_COLLAPSED_CARD_HEIGHT}px` } : undefined}
      data-ctx="article"
      data-id={item.id}
      role="article"
      data-card-index={cardIndex}
      tabIndex={tabbable ? 0 : -1}
      aria-current={selected ? 'true' : undefined}
      onKeyDown={(e) => {
        if (activePlayerOwnsSpace(e)) return;
        if (e.key === 'Enter' || e.key === ' ') {
          if (e.target === e.currentTarget) {
            e.preventDefault();
            onSelect(item.id);
          }
          return;
        }
        if (e.key === 'ArrowDown' || e.key === 'ArrowRight') { e.preventDefault(); onMoveFocus(cardIndex, 1); }
        else if (e.key === 'ArrowUp' || e.key === 'ArrowLeft') { e.preventDefault(); onMoveFocus(cardIndex, -1); }
      }}
    >
      <div className="social-avatar">{feedName.charAt(0) || '?'}</div>
      <div className="social-body">
        {/* 标题：社交布局此前漏显示——正文太长时一眼无法辨识内容主题 */}
        {item.title && <div className="social-card-title" title={item.title}>{item.title}</div>}
        <div className="social-author-row">
          <strong className="social-author-name">{item.author}</strong>
          <span className="social-handle">{feedName}</span>
          <span className="social-date">{formatRelativeTime(item.publishedAt)}</span>
        </div>
        {/* 正文是消毒后的 HTML（同 Reader）；水合完成前显示轻量占位（毫秒级）。
            <img> 点击走灯箱放大（与 Reader 一致），<a> 走外链。
            TASK-122：正文与三态从 body 派生——content 优先；failed=内联重试；
            missing=文章不存在；ready/cleared（空正文终态，正文保留不受 AI 清理
            影响）=「暂无正文」；其余（loading/未请求）=加载占位。 */}
        <div
          ref={textRef}
          id={`social-text-${item.id}`}
          className={`social-text ${expanded ? '' : 'collapsed'} ${hydratingBody ? 'social-text-loading' : ''} ${currentMeasurement?.long && !expanded ? 'social-text-clipped' : ''}`}
          style={expanded ? undefined : { height: `${SOCIAL_COLLAPSED_BODY_HEIGHT}px` }}
          onClick={(e) => {
            const target = e.target as HTMLElement;
            if (target.tagName === 'IMG') {
              const src = (target as HTMLImageElement).currentSrc || (target as HTMLImageElement).src;
              if (src && /^https?:\/\//.test(src)) {
                e.preventDefault();
                openLightbox(src, undefined, item.url);
              }
              return;
            }
            handleArticleLinkClick(e);
          }}
        >
          <div ref={innerRef} className="social-text-inner">
            {body.content ? (
              <div dangerouslySetInnerHTML={contentHtml} />
            ) : body.state === 'failed' ? (
              <HydrateFailure
                message={body.message}
                onRetry={() => useAppStore.getState().retryHydration(item.id)}
              />
            ) : body.state === 'missing' ? (
              <span className="hydrate-placeholder">{body.message}</span>
            ) : body.state === 'ready' || body.state === 'cleared' ? (
              /* TASK-100：占位透明度并入 .hydrate-placeholder 类（此前内联 opacity 两处） */
              <span className="hydrate-placeholder">暂无正文</span>
            ) : (
              <span className="hydrate-placeholder">加载正文…</span>
            )}
          </div>
        </div>
        <div className="social-expand-slot">
          {currentMeasurement?.long && (
            <button
              className="notif-expand-btn social-expand-btn"
              aria-expanded={expanded}
              aria-controls={`social-text-${item.id}`}
              onClick={() => setExpanded(!expanded)}
            >
              {expanded ? '收起内容 ▲' : '展开更多 ▼'}
            </button>
          )}
        </div>
        <div className={"social-translated-block" + (showTranslate ? " show" : "")} onClick={handleArticleLinkClick}>
          {/* TASK-065 N8/N11：未消毒流式产物按纯文本渲染；消毒后与 Reader 同口径按 HTML 渲染。
              TASK-122：cleared 判别态呈现「已清空」（与「从未生成过」的空白区分） */}
          {rawTranslated ? (
            <span>{body.translatedContent}</span>
          ) : body.state === 'cleared' && !body.translatedContent ? (
            <span className="hydrate-placeholder">AI 缓存已清空，可重新生成</span>
          ) : (
            <span dangerouslySetInnerHTML={translatedHtml} />
          )}
          {translatingCard ? <span>翻译中…</span> : null}
          {/* fix-5：翻译失败内联错误行 + 重试（此前失败只有 toast 一闪，半截译文无恢复入口） */}
          {translateError && !translatingCard ? (
            <div className="ai-error-row">
              <span className="ai-error-text" title={translateError}>翻译失败：{translateError}</span>
              <button className="ai-retry-btn" onClick={() => useAppStore.getState().translateEntry(item.id)}>重试</button>
            </div>
          ) : null}
          {/* OPT-013 B3：五态入口——stale：「已过期」+ 重新生成；partial：
              「部分完成（N/M 段）」+ 继续（复用已完成片段）+ 清除（删后端进度）。 */}
          {translationState === 'stale' ? (
            <div className="ai-state-row">
              <span className="ai-unverified-tag" title="该译文与当前模型/提示词/正文不匹配（或没有缓存签名）——重新生成后更新为已验证版本">
                已过期
              </span>
              <button className="ai-retry-btn" onClick={() => useAppStore.getState().translateEntry(item.id)}>重新生成</button>
            </div>
          ) : null}
          {translationState === 'partial' && translationSegments ? (
            <div className="ai-state-row">
              <span className="ai-partial-hint">
                部分完成（已完成 {translationSegments.completed}/{translationSegments.total} 段）
              </span>
              <button className="ai-retry-btn" onClick={() => useAppStore.getState().translateEntry(item.id)}>继续</button>
              <button
                className="ai-retry-btn"
                title="删除本机保存的全部分段进度与整篇译文，之后重新生成从零开始"
                onClick={() => void useAppStore.getState().clearAiProduct(item.id, 'translation')}
              >
                清除
              </button>
            </div>
          ) : null}
        </div>
        <div className="social-actions-bar">
          <button
            className={`social-act-item ${item.isStarred ? 'starred' : ''}`}
            onClick={() => {
              toggleEntryFlag(item.id, 'isStarred');
            }}
          >
            {/* TASK-100 U3：星标视觉统一——收藏态用 starFilled（与 Reader/右键菜单同源） */}
            {item.isStarred ? <Icons.starFilled /> : <Icons.star />}
            <span>{item.isStarred ? '取消收藏' : '收藏'}</span>
          </button>
          <button
            className={`social-act-item ${item.isRead ? 'act-on' : ''}`}
            onClick={() => {
              toggleEntryFlag(item.id, 'isRead');
            }}
          >
            <Icons.check />
            <span>{item.isRead ? '标为未读' : '标为已读'}</span>
          </button>
          <button
            className={`social-act-item ${showTranslate ? 'active-translate' : ''}`}
            onClick={() => {
              const next = !showTranslate;
              /* fix-5：失败态（translateErrors[id] 存在）时点「翻译」必须重走
                 translateEntry 重试——旧逻辑在有半截译文时会把它当缓存只切显示，
                 重试按钮变成死路径（P2-3）。
                 OPT-013 B1：已有译文时也必须走 translateEntry 的命中判定（后端
                 验签名）——未验证（无签名/已过期）会重新生成，旧产物保留显示；
                 已验证则无操作（不重复计费）。卡片的显隐切换仍由本按钮负责。 */
              if (next && (translateError || !body.translatedContent)) {
                /* 无译文或上次失败：实际触发生成/重试（P1-7 空壳修复 + fix-5） */
                useAppStore.getState().translateEntry(item.id);
              } else if (body.translatedContent) {
                if (next) useAppStore.getState().translateEntry(item.id);
              }
              setTransOverride(next);
            }}
          >
            <Icons.globe />
            <span>翻译</span>
          </button>
          <button
            className="social-act-item"
            onClick={() => {
              if (!item.url) { showToast('该条目没有原文链接'); return; }
              void openExternalWithFeedback(item.url);
            }}
          >
            <Icons.externalLink />
            <span>查看原文</span>
          </button>
        </div>
      </div>
    </div>
  );
});

/* ---------- 画廊卡片 ---------- */

const GalleryCard = memo(function GalleryCard({ item, cardIndex, tabbable, lanes, onMoveFocus }: {
  item: ArticleEntry;
  cardIndex: number;
  tabbable: boolean;
  lanes: number;
  onMoveFocus: (from: number, delta: number) => void;
}) {
  const toggleEntryFlag = useAppStore((s) => s.toggleEntryFlag);
  const openGalleryLightbox = useAppStore((s) => s.openGalleryLightbox);
  const selectArticle = useAppStore((s) => s.selectArticle);
  const activeArticleId = useAppStore((s) => s.activeArticleId);
  const selected = activeArticleId === item.id;
  const feedName = useAppStore((s) => (s.sourceIndex.get(item.feedId) ?? s.feedIndex.get(item.feedId))?.feed.name ?? '');
  const pendingGallery = useAppStore((s) => s.lightboxGallery?.entryId === item.id && !s.lightboxUrl
    ? s.lightboxGallery.status : null);
  const mediaKey = galleryMediaKey(item);
  const busy = useAppStore(s => s.libraryRestoreBusy);
  const generation = useAppStore(s => s.libraryGeneration);
  const { lease, snapshot, retry } = useImageResource(item.imageUrl, item.url, !busy, 1, `${generation}:${mediaKey}`);
  const direct = !item.imageUrl || !canProxyImages() || !needsImageProxy(item.imageUrl);
  const imgSrc = snapshot.status === 'ready' ? snapshot.src : direct ? item.imageUrl : null;
  const imgFailed = snapshot.status === 'failed';
  const openImage = (target: HTMLElement) => {
    // 图片失败/换源时触发节点也不卸载；useFocusTrap 归还同一卡片且 preventScroll。
    target.focus({ preventScroll: true });
    if (imgFailed) retry();
    selectArticle(item.id);
    openGalleryLightbox(item.id);
  };
  const onImgError = () => lease?.failDecode();
  const imageCount = item.imageCount ?? (item.imageUrl ? 1 : 0);
  return (
    <div className={`gallery-card ${item.isRead ? 'read' : ''} ${selected ? 'active-selected' : ''}`} data-ctx="article" data-id={item.id}>
      <div className="gallery-media">
        <div
          className="gallery-media-trigger"
          onClick={(e) => openImage(e.currentTarget)}
          role="button"
          data-card-index={cardIndex}
          tabIndex={tabbable ? 0 : -1}
          aria-label={`预览${item.title}，${imageCount} 张图片`}
          aria-busy={pendingGallery === 'loading'}
          aria-current={selected ? 'true' : undefined}
          onKeyDown={(e) => {
            if (activePlayerOwnsSpace(e)) return;
            if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); openImage(e.currentTarget); return; }
            /* 行优先 lanes 的几何语义（C-P2-2）：Right/Left = 同排相邻 ±1、
               Down/Up = 同列跨排 ±列数。边界 clamp：目标越界（首尾卡）不动——
               不跳列、不回绕（moveCardFocus 的越界 no-op 即 clamp，最小惊讶）。
               贪心最短列分配在卡片高度不均时会偏离严格行列网格，±列数是同列
               语义的工程近似。Note: 见 .agents/notes/implemented/architecture/2026-11-13-界面布局与启动时序.md */
            if (e.key === 'ArrowRight') { e.preventDefault(); onMoveFocus(cardIndex, 1); }
            else if (e.key === 'ArrowLeft') { e.preventDefault(); onMoveFocus(cardIndex, -1); }
            else if (e.key === 'ArrowDown') { e.preventDefault(); onMoveFocus(cardIndex, lanes); }
            else if (e.key === 'ArrowUp') { e.preventDefault(); onMoveFocus(cardIndex, -lanes); }
          }}
        >
          {imgSrc && !imgFailed ? (
            <img key={`${mediaKey}:${imgSrc}`} src={imgSrc} loading="lazy" onError={onImgError}
              alt={item.title} referrerPolicy="no-referrer" />
          ) : (
            <div className={`gallery-no-image${imgFailed ? ' cover-fallback' : ''}`}
              data-cover-state={imgFailed ? 'failed' : 'empty'}>
              {pendingGallery === 'loading' || (!direct && !imgFailed && !imgSrc) ? '加载图片…'
                : pendingGallery === 'error' ? '图片加载失败，点击重试'
                : pendingGallery === 'empty' ? '暂无图片，点击重试'
                : imgFailed ? <span>图片加载失败，点击重试</span> : '无图'}
            </div>
          )}
        </div>
        <button className="gallery-image-count" tabIndex={tabbable ? 0 : -1}
          aria-label={`查看全部 ${imageCount} 张图片`} onClick={(e) => openImage(e.currentTarget)}>
          <Icons.image /><span>{imageCount}</span>
        </button>
      </div>
      <div className="gallery-meta">
        {/* TASK-100 U5：截断的画廊标题补 title（=未截断全文） */}
        <div className="gallery-title" title={item.title}>{item.title}</div>
        <div className="gallery-meta-row">
          <span>{feedName}</span>
          <div style={{ display: 'flex', gap: 6 }}>
            <button
              className={`toggle-action-btn notif-act ${item.isStarred ? 'act-on' : ''}`}
              onClick={(e) => { e.stopPropagation(); toggleEntryFlag(item.id, 'isStarred'); }}
              tabIndex={tabbable ? 0 : -1}
              title={item.isStarred ? '取消收藏' : '收藏'}
            >
              {/* TASK-100 U3：星号/空心星字符改 Icons SVG（收藏态 starFilled），颜色仍走 --star-color */}
              <span style={{ color: item.isStarred ? 'var(--star-color)' : 'inherit', display: 'inline-flex' }}>
                {item.isStarred ? <Icons.starFilled /> : <Icons.star />}
              </span>
            </button>
            <button
              className={`toggle-action-btn notif-act ${item.isRead ? 'act-on' : ''}`}
              onClick={(e) => { e.stopPropagation(); toggleEntryFlag(item.id, 'isRead'); }}
              tabIndex={tabbable ? 0 : -1}
            >
              <span>{item.isRead ? '标为未读' : '标为已读'}</span>
            </button>
          </div>
        </div>
      </div>
    </div>
  );
});

/* ---------- 播客卡片 ---------- */

const PodcastCard = memo(function PodcastCard({ item, cardIndex, tabbable, onMoveFocus }: {
  item: ArticleEntry;
  cardIndex: number;
  tabbable: boolean;
  onMoveFocus: (from: number, delta: number) => void;
}) {
  const playPodcastEpisode = useAppStore((s) => s.playPodcastEpisode);
  const togglePlayerPlay = useAppStore((s) => s.togglePlayerPlay);
  const activeArticleId = useAppStore((s) => s.activeArticleId);
  const selected = activeArticleId === item.id;
  const feedName = useAppStore((s) => (s.sourceIndex.get(item.feedId) ?? s.feedIndex.get(item.feedId))?.feed.name ?? '');
  const play = () => {
    const audioUrl = item.enclosureUrl ?? '';
    const cur = useAppStore.getState().player;
    /* P3[F3]（REQ-104）：判据取自纯函数 `podcastClickAction`，定义在
       src/store/selectors.ts（放在那里是为了避开 oxlint 的 react/only-export-components
       约束——组件文件 export 非组件函数会被判 Fast Refresh 违规，与 timelineSentinel.ts
       同一考虑），本组件与前端回归网消费的是**同一份**判定。
       证据边界（如实说明，审查 FINDING TASK-081-R2-F1）：回归网对**该纯函数**有变异取证
       （改回无条件 play → 2 条断言失败）；对**本组件是否真的调用了它**，由回归网里的
       源码形态断言（检查本处 if 分支存在且调用点带 === 'toggle'）覆盖，而非 DOM 点击。 */
    if (podcastClickAction(cur.isActive, cur.audioUrl, audioUrl) === 'toggle') {
      togglePlayerPlay();
      return;
    }
    playPodcastEpisode(item.title, feedName, item.cover ?? '', audioUrl, item.id);
  };
  return (
    <div
      className={`podcast-card ${item.isRead ? 'read' : ''} ${selected ? 'active-selected' : ''}`}
      onClick={play}
      role="button"
      data-card-index={cardIndex}
      tabIndex={tabbable ? 0 : -1}
      aria-current={selected ? 'true' : undefined}
      onKeyDown={(e) => {
        if (activePlayerOwnsSpace(e)) return;
        if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); play(); return; }
        if (e.key === 'ArrowDown' || e.key === 'ArrowRight') { e.preventDefault(); onMoveFocus(cardIndex, 1); }
        else if (e.key === 'ArrowUp' || e.key === 'ArrowLeft') { e.preventDefault(); onMoveFocus(cardIndex, -1); }
      }}
      data-ctx="article"
      data-id={item.id}
    >
      {/* 封面：共享 CoverImage；无 cover 时出同尺寸占位（不再渲染无 src 的 img） */}
      <CoverImage
        src={item.cover}
        articleId={item.id}
        pageUrl={item.url}
        className="podcast-cover-box"
        alt="cover"
        loading="lazy"
        empty={<div className="podcast-cover-box cover-fallback" data-cover-state="empty" aria-hidden="true" />}
      />
      <div style={{ flex: 1 }}>
        <div className="podcast-show-name">
          {feedName}
          {item.durationSec != null && ` · ${formatDuration(item.durationSec)}`}
        </div>
        <div className="podcast-title" title={item.title}>{item.title}</div>
        <div className="podcast-desc">{item.snippet}</div>
      </div>
      <div className="podcast-play-circle">
        <Icons.play />
      </div>
    </div>
  );
});

/* ---------- 通知卡片 ---------- */

const NotifCard = memo(function NotifCard({ item, onSelect, cardIndex, tabbable, onMoveFocus }: {
  item: ArticleEntry;
  onSelect: (id: string) => void;
  cardIndex: number;
  tabbable: boolean;
  onMoveFocus: (from: number, delta: number) => void;
}) {
  const feedConfig = useAppStore(useShallow((s) => selectFeedConfig(s, item.feedId)));
  const activeArticleId = useAppStore((s) => s.activeArticleId);
  const selected = activeArticleId === item.id;
  const toggleEntryFlag = useAppStore((s) => s.toggleEntryFlag);
  const summarizeEntry = useAppStore((s) => s.summarizeEntry);
  const feedName = useAppStore((s) => (s.sourceIndex.get(item.feedId) ?? s.feedIndex.get(item.feedId))?.feed.name ?? '');
  const summaryGenerating = useAppStore((s) => s.summarizingIds[item.id]);
  const summaryError = useAppStore((s) => s.summaryErrors[item.id] || '');
  /* OPT-013 B3：摘要与译文五态（卡片入口与 Reader 同一单点判定）。 */
  const summaryState = useAppStore((s) => selectAiProductState(s, item.id, 'summary'));
  const translationState = useAppStore((s) => selectAiProductState(s, item.id, 'translation'));
  const translationSegments = useAppStore((s) => selectAiCacheStatus(s, item.id)?.translationSegments ?? null);
  const aiStatusInput = useAppStore(useShallow((s) => selectAiStatusInput(s, item.id)));
  const [summaryOverride, setSummaryOverride] = useState<boolean | null>(null);
  const [transOverride, setTransOverride] = useState<boolean | null>(null);
  const translatingCard = useAppStore((s) => s.translatingIds[item.id]);
  /* TASK-065 N11：同 SocialCard——未消毒流式产物按纯文本渲染 */
  const rawTranslated = useAppStore((s) => s.rawTranslatedIds[item.id]);
  const [expanded, setExpanded] = useState(false);
  /* fix-5：卡片级翻译失败信息（内联错误行 + 重试依据） */
  const translateError = useAppStore((s) => s.translateErrors[item.id] || '');
  /* 进入视口附近才水合全文（与社交卡一致）：列表快照的 snippet 是 280 字截断，
     「展开更多」必须展示全文而非同一段截断文本 */
  const hydrateRef = useLazyHydrate(item.id);
  useCenterDwellAi(item.id, hydrateRef);
  /* TASK-114 X1 → TASK-122：水合三态从 selectArticleBody 派生（对齐 SocialCard）
     ——useLazyHydrate 内部已按 entryNeedsHydration 触发请求，这里消费记录状态
     把结果呈现出来：失败=内联重试（不再静默回退 snippet）、missing=文章不存在、
     终态空正文=暂无正文、其余=加载占位。请求与 SocialCard 走同一条批量水合
     队列（enqueueHydration）。
     【为何必须包 useShallow】同 SocialCard：selectArticleBody 返回新对象，
     zustand v5 + useSyncExternalStore 无快照缓存会无限重渲染（React #185）。 */
  const body = useAppStore(useShallow((s) => selectArticleBody(s, item.id)));
  /* 展示文本：展开态优先水合全文（剥 HTML 标签），未水合/收起态用 snippet */
  const fullText = body.content
    ? body.content.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim()
    : '';
  const displayText = expanded && fullText ? fullText : item.snippet;
  /* 失败后卡片保持展开（展示错误 + 重试按钮） */
  /* fix-8：摘要框跟随 auto 配置展开的前提加「有产物/在途/出错」——auto 开但
     尚无摘要时按收起处理，不再渲染空壳摘要框（自动生成仅在中心连续停顿
     时触发，卡片挂载本身不发起）；手动点「摘要」仍会就地
     触发生成并展开。 */
  const summaryOpen = summaryOverride ?? autoAiBlockOpen(
    feedConfig.autoSummary,
    !!body.aiSummary,
    summaryGenerating,
    !!summaryError,
  );
  /* fix-8：译文块同口径（见上） */
  const transShow = transOverride ?? (translationState === 'partial' || autoAiBlockOpen(
    feedConfig.autoTranslate,
    !!body.translatedContent,
    !!translatingCard,
    !!translateError,
  ));
  /* 自动收起：按展示源文本判定（全文可得时按全文长度，否则按 snippet），
     短内容直接全文展示、不渲染展开按钮 */
  const isLong = (fullText || item.snippet || '').length > 120;

  /* 与社交卡共用输入资格；仅片段重启也能发现 partial，不依赖先显示整篇产物。 */
  useEffect(() => {
    if (body.state === 'unrequested' || body.state === 'loading') return;
    useAppStore.getState().ensureAiStatus(item.id);
  }, [body.state, aiStatusInput, item.id]);

  return (
    /* TASK-100（UI P2-2 轻修）：同 SocialCard——补 role/tabIndex 融入 roving 体系。
       TASK-114 X2：Enter/Space=选中，e.target 守卫与 role 裁定同 SocialCard 注释。 */
    <div
      ref={hydrateRef}
      className={`notif-card ${item.isRead ? 'read' : ''} ${selected ? 'active-selected' : ''}`}
      data-ctx="article"
      data-id={item.id}
      role="article"
      data-card-index={cardIndex}
      tabIndex={tabbable ? 0 : -1}
      aria-current={selected ? 'true' : undefined}
      onKeyDown={(e) => {
        if (activePlayerOwnsSpace(e)) return;
        if (e.key === 'Enter' || e.key === ' ') {
          if (e.target === e.currentTarget) {
            e.preventDefault();
            onSelect(item.id);
          }
          return;
        }
        if (e.key === 'ArrowDown' || e.key === 'ArrowRight') { e.preventDefault(); onMoveFocus(cardIndex, 1); }
        else if (e.key === 'ArrowUp' || e.key === 'ArrowLeft') { e.preventDefault(); onMoveFocus(cardIndex, -1); }
      }}
    >
      <div className="notif-card-header-row">
        <div className="notif-title">{item.title}</div>
        <div className="notif-top-actions">
          <button
            className={`toggle-action-btn notif-act ${summaryOpen ? 'act-on' : ''}`}
            onClick={() => {
              /* 未开自动摘要的源默认不显示卡片：点开后就地触发生成（有缓存直接展示） */
              if (!summaryOpen) summarizeEntry(item.id);
              setSummaryOverride(!summaryOpen);
            }}
          >
            <Icons.spark />
            <span>摘要</span>
          </button>
          <button
            className={`toggle-action-btn notif-act ${transShow ? 'act-on' : ''}`}
            onClick={() => {
              const next = !transShow;
              /* fix-5：失败态时点「翻译」必须重走 translateEntry 重试（与
                 SocialCard 同口径，半截译文不再被当成缓存只切显示） */
              if (next && (translateError || !body.translatedContent)) {
                useAppStore.getState().translateEntry(item.id);
              }
              setTransOverride(next);
            }}
          >
            <Icons.globe />
            <span>翻译</span>
          </button>
          <button
            className={`toggle-action-btn notif-act ${item.isRead ? 'act-on' : ''}`}
            onClick={() => toggleEntryFlag(item.id, 'isRead')}
          >
            <Icons.check />
            <span>{item.isRead ? '标为未读' : '标为已读'}</span>
          </button>
        </div>
      </div>

      <div className="notif-meta-row">
        {feedName} · {formatRelativeTime(item.publishedAt)}
      </div>

      <div className={`notif-ai-box ${summaryOpen ? 'open' : ''}`}>
        <div className="notif-ai-label">
          <Icons.spark />
          <span>摘要</span>
          {/* OPT-013 B3：摘要 stale →「已过期」+ 重新生成入口（真实调用后端）。 */}
          {summaryState === 'stale' ? (
            <>
              <span className="ai-unverified-tag" title="该摘要与当前模型/提示词/正文不匹配（或没有缓存签名）——重新生成后更新为已验证版本">
                已过期
              </span>
              <button className="ai-retry-btn" onClick={() => summarizeEntry(item.id)}>重新生成</button>
            </>
          ) : null}
        </div>
        {summaryError ? (
          <div className="ai-error-row">
            {/* TASK-100：错误前缀统一「摘要生成失败：」（此前 Reader/通知卡两种写法） */}
            <span className="ai-error-text" title={summaryError}>摘要生成失败：{summaryError}</span>
            <button className="ai-retry-btn" onClick={() => summarizeEntry(item.id)}>重试</button>
          </div>
        ) : summaryGenerating && !body.aiSummary ? (
          <div className="notif-ai-text ai-generating-hint">正在生成摘要…</div>
        ) : body.state === 'cleared' && !body.aiSummary ? (
          /* TASK-122：cleared 判别态呈现「已清空」（与「从未生成过」的空白区分） */
          <div className="notif-ai-text ai-generating-hint">AI 缓存已清空，可重新生成</div>
        ) : (
          <div className="notif-ai-text">{body.aiSummary}</div>
        )}
      </div>

      {/* TASK-114 X1：水合三态对齐 SocialCard（同构：正文→正文 / 错误→内联重试 /
          终态空→暂无正文 / 其余→加载占位，复用 .hydrate-retry / .hydrate-placeholder
          同一套样式）。
          TASK-114 R1-F1：正文分支（fullText）置于失败分支**之前**，与基准 SocialCard
          的 content 优先逐分支对齐——ensureArticleContent 详情拉取成功只写
          bodyById 记录的 content、从不清 failed 态（reader.ts 详情 .then 分支），
          「错误态 + 正文已到达」是可达组合态（列表批量水合失败 → Enter/J-K 打开
          → 详情成功），该状态下必须显示已到达的正文，而非把正文替换成假的失败
          重试行。
          失败分支只覆盖「无正文可显示」的失败：snippet 回退保持在错误**之后**
          （契约「失败不再静默回退 snippet」）；非失败态保留 snippet 展示（通知卡的
          主正文本就是 snippet，加载窗口内把可读内容换成占位是信息损失）。 */}
      {fullText ? (
        <div className={`notif-body-text ${isLong && !expanded ? 'collapsed' : ''}`}>{displayText}</div>
      ) : body.state === 'failed' ? (
        <HydrateFailure
          message={body.message}
          onRetry={() => useAppStore.getState().retryHydration(item.id)}
        />
      ) : item.snippet ? (
        <div className={`notif-body-text ${isLong && !expanded ? 'collapsed' : ''}`}>{displayText}</div>
      ) : body.state === 'missing' ? (
        <div className="notif-body-text"><span className="hydrate-placeholder">{body.message}</span></div>
      ) : body.state === 'ready' || body.state === 'cleared' ? (
        <div className="notif-body-text"><span className="hydrate-placeholder">暂无正文</span></div>
      ) : (
        <div className="notif-body-text"><span className="hydrate-placeholder">加载正文…</span></div>
      )}

      <div className={`notif-translated-block ${transShow ? 'show' : ''}`} onClick={handleArticleLinkClick}>
        {/* TASK-065 N8/N11：同 SocialCard——未消毒按纯文本，消毒后按 HTML */}
        {rawTranslated ? (
          <span>{body.translatedContent}</span>
        ) : body.state === 'cleared' && !body.translatedContent ? (
          /* TASK-122：cleared 判别态呈现「已清空」 */
          <span className="hydrate-placeholder">AI 缓存已清空，可重新生成</span>
        ) : (
          <span dangerouslySetInnerHTML={{ __html: body.translatedContent }} />
        )}
        {translatingCard ? <span>翻译中…</span> : null}
        {/* fix-5：翻译失败内联错误行 + 重试（与 SocialCard 同形态） */}
        {translateError && !translatingCard ? (
          <div className="ai-error-row">
            <span className="ai-error-text" title={translateError}>翻译失败：{translateError}</span>
            <button className="ai-retry-btn" onClick={() => useAppStore.getState().translateEntry(item.id)}>重试</button>
          </div>
        ) : null}
        {/* OPT-013 B3：译文五态入口（与 SocialCard 同形态——stale/partial）。 */}
        {translationState === 'stale' ? (
          <div className="ai-state-row">
            <span className="ai-unverified-tag" title="该译文与当前模型/提示词/正文不匹配（或没有缓存签名）——重新生成后更新为已验证版本">
              已过期
            </span>
            <button className="ai-retry-btn" onClick={() => useAppStore.getState().translateEntry(item.id)}>重新生成</button>
          </div>
        ) : null}
        {translationState === 'partial' && translationSegments ? (
          <div className="ai-state-row">
            <span className="ai-partial-hint">
              部分完成（已完成 {translationSegments.completed}/{translationSegments.total} 段）
            </span>
            <button className="ai-retry-btn" onClick={() => useAppStore.getState().translateEntry(item.id)}>继续</button>
            <button
              className="ai-retry-btn"
              title="删除本机保存的全部分段进度与整篇译文，之后重新生成从零开始"
              onClick={() => void useAppStore.getState().clearAiProduct(item.id, 'translation')}
            >
              清除
            </button>
          </div>
        ) : null}
      </div>

      {/* TASK-114 X1：纯失败态（无正文可显示）正文已被重试行替换，展开按钮不渲染
          （切换无对象，避免死控件）。TASK-114 R2：错误态滞留 + 正文经详情路到达
          （reader.ts 详情 .then 只写 content 不清错误）的组合态下，正文分支显示的是
          收起态 2 行钳制的 snippet——与修前及同态 SocialCard 一致保留「展开更多」
          （此时展开有对象：displayText 切到水合全文），故门控对 !!fullText 豁免。 */}
      {isLong && (body.state !== 'failed' || !!fullText) && (
        <button className="notif-expand-btn" onClick={() => setExpanded(!expanded)}>
          {expanded ? '收起内容 ▲' : '展开更多 ▼'}
        </button>
      )}
    </div>
  );
});


