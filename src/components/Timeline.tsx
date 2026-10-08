import { memo, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { scrollAwayRange, isUserScrollEvent, PROGRAMMATIC_SCROLL_SUPPRESS_MS } from './scrollAwayRead';
import { useVirtualizer } from '@tanstack/react-virtual';
import { useShallow } from 'zustand/react/shallow';
import {
  useAppStore,
  LAYOUT_NAMES,
  VIEW_NAMES,
  podcastClickAction,
  autoAiBlockOpen,
  entryNeedsHydration,
  selectArticleBody,
  selectVisibleEntries,
  selectFeedConfig,
} from '../store';
import { Icons } from './icons';
import { formatRelativeTime, formatDuration } from '../lib/format';
import { openExternal, handleArticleLinkClick } from '../lib/external';
import { proxyImageUrl } from '../lib/imageProxy';
import { onCoverError } from '../lib/coverImage';
import { CoverImage } from './CoverImage';
import type { ArticleEntry } from '../types';
import { useEnteringClass } from './useEnteringClass';
import { sentinelMode } from './timelineSentinel';
import { refillDecision } from './timelineRefill';
import { anchorRestoreIndex, clearTopAnchor, ANCHOR_RECORD_THROTTLE_MS, commitTopAnchor, peekReturnAnchor, peekTopAnchor, recordTopAnchor, readerFocusReturnIndex, rearmTopAnchor, stashTopAnchorForReturn } from './timelineAnchor';

/* ============================================================
   Timeline —— 顶栏（标题/筛选/排序/全部已读）+ 五布局渲染器

   交互设计：
   - 列表容器在布局/视图/筛选切换时做一次 160ms 的淡入过渡，
     避免内容瞬间替换造成的视觉跳动（"闪一下"）。
   - 列表切换后滚动位置归零（新列表从顶部阅读）。
     TASK-115 例外：切布局/视图/范围后**切回**且缓存命中时，按 per-filterKey
     锚存档恢复到离开时的位置（返回位置统一规则，见 timelineAnchor.ts
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
  /* TASK-115②：最后选中的文章 id（关闭信号消费侧的归还目标）。activeArticleId
     跟随 effect 顺带记账——关闭阅读器的时刻它必然持有阅读器正在显示的文章
     （关闭后置 null 的分支不更新，ref 保留关闭前的值）。选型理由见
     timelineAnchor.ts 头注 X2 节（store 不反向依赖 components 层）。 */
  const lastActiveArticleIdRef = useRef<string | null>(null);
  useEffect(() => {
    if (!activeArticleId) return;
    lastActiveArticleIdRef.current = activeArticleId;
    const idx = items.findIndex((a) => a.id === activeArticleId);
    if (idx >= 0) setFocusIndex(idx);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeArticleId]);

  /* 方向键在卡片间移动：目标可能未被虚拟化渲染，先 scrollToIndex 再于下一帧聚焦 */
  /* TASK-115②：定位-聚焦动作收口为 focusCardAt（绝对下标），焦点归还与方向键
     移动共用同一套「setFocusIndex → 程序性滚动抑制 → scrollToIndex(align:auto)
     → 下一帧聚焦」——align:auto 语义正好满足契约「原卡可见不滚、不可见才
     scrollToIndex 定位」，两种入口不再各写一份。 */
  const focusCardAt = (index: number) => {
    setFocusIndex(index);
    suppressNextScrollEvents();
    rowVirtualizer.scrollToIndex(index, { align: 'auto' });
    requestAnimationFrame(() => {
      const root = scrollRef.current;
      const el = root?.querySelector<HTMLElement>(`[data-card-index="${index}"]`);
      el?.focus();
    });
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

  /* 筛选上下文变化 → 滚动归零。不再用 key 重挂载整个列表 DOM（此前每次
     布局/视图/排序切换都强制卸载重建全部卡片 + 重建全部 IntersectionObserver/
     ResizeObserver，是「切换卡顿 + 加载正文闪动」的主因）；改为复用 DOM，
     React 只 diff 列表项，内容切换即时可见。 */
  const filterKey = `${activeContentLayout}|${activeViewFilter}|${activeFeedFilter}|${timelineFilter}|${timelineSort}`;

  useEffect(() => {
    /* 程序性归零：先开抑制窗口再滚，随后到达的 scroll 事件不算用户滚动 */
    suppressNextScrollEvents();
    document.getElementById('timelineContentScroll')?.scrollTo({ top: 0 });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filterKey]);

  /* ---------- 滚动出列表视口 → 标已读（markReadOnScrollOut） ---------- */
  const scrollRef = useRef<HTMLDivElement>(null);
  /* 布局/视图/筛选/排序切换时列表淡入一次（REQ-005）。此前 .list-entering 在
     CSS 里声明了却从未被应用，是死代码。 */
  useEnteringClass(
    scrollRef,
    `${activeContentLayout}|${activeViewFilter}|${activeFeedFilter}|${timelineFilter}|${timelineSort}`,
    'list-entering',
  );

  /* 虚拟滚动：只渲染视口 + overscan 缓冲内的条目（约 30 条），滚动时复用 DOM，
     彻底消除「一次性渲染 500 张含正文卡片」的卡顿（成熟 RSS 客户端的共识做法）。 */
  const rowVirtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 160,
    overscan: 8,
    getItemKey: (index) => items[index]?.id ?? index,
    /* 画廊（瀑布流多列）不虚拟化；其余单列布局虚拟化。 */
    enabled: activeContentLayout !== 'image',
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
     timelineAnchor.ts 头注①）。px≤0 不产生额外滚动；补偏发生在程序性滚动
     抑制窗口内，不会被误判为用户滚动。 */
  const applyAnchorOffsetPx = (px: number) => {
    if (px <= 0) return;
    const el = scrollRef.current;
    if (el) el.scrollTop += px;
  };
  /* TASK-123①：顶条卡片内像素偏移的统一测量（scrollTop − 顶条卡片虚拟起点）。
     记录（recordTopAnchor）与尾沿补记（commitTopAnchor）共用同一口径，保证
     锚载荷在两条写入路径下语义一致。画廊布局（虚拟化禁用）不调用。 */
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
      if (live.activeContentLayout === 'image') return; // 画廊非虚拟化，与记录路径同口径回落
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
       timelineAnchor.ts 头注）。 */
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

  /* 选中文章（搜索/命令面板/J/K 导航）→ 滚动定位到该卡片。虚拟化下卡片
     可能不在可视区（不渲染），不能用 scrollIntoView；改用 virtualizer 的
     scrollToIndex。
     align:'auto' 只在目标不完全可见时才滚动，点击可见卡片不会抖动。
     只依赖 activeArticleId：定位是「选中文章变化」这个动作的副作用，必须
     只在此时触发一次。此前依赖 totalSize/items（为了在动态测量后微调），
     但副作用是——用户手动滚动列表时，滚动触发 measureElement → totalSize 变、
     或滚动到底触发 loadMoreArticles → items 变，都会重新 scrollToIndex 把列表
     拉回选中文章（「列表滚一会又跳回原位」的根因）。动态高度下首次定位的
     少量误差可接受（估算行高），远好于「无法自由滚动」。 */
  useEffect(() => {
    if (!activeArticleId) return;
    const idx = items.findIndex((a) => a.id === activeArticleId);
    if (idx < 0) return; // 目标不在当前 items（如 anchorToArticle 异步窗口），跳过
    /* 程序性定位（J/K / 搜索锚定 / focusIndex 跟随）：先开抑制窗口再滚——
       K 在顶部回绕到末项时 startIndex 0→N，修前会被当成用户滚动整段标读 */
    suppressNextScrollEvents();
    rowVirtualizer.scrollToIndex(idx, { align: 'auto' });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeArticleId]);

  /* TASK-111②：后台刷新保位——消费顶条锚。reloadFromBackend 以
     keepReadingPosition 落地（feeds-updated / 手动同步 / 单源刷新路径）时
     store bump positionRestoreNonce，本 effect 消费锚：锚 id 仍在新快照中 →
     程序性滚动回其新索引（新文章插入头部场景，抵消整体替换 + 头部插入造成的
     视觉跳动）；无锚 / filterKey 已变 / 锚 id 不在快照 → 回落现状（不强制顶部）。
     分流语义：
     - 锚与 filterKey 一起记账（timelineAnchor），消费时逐字比对——用户主动
       切范围/布局/视图/排序后锚已被 clearTopAnchor 丢弃且比对也不通过，
       导航路径的 reload 亦不 bump nonce（三重隔离）；
     - 复用 activeArticleId 定位 effect 的既有程序性滚动抑制机制
       （suppressNextScrollEvents → scrollToIndex，防回位被误判成用户滚动
       而整段标读）；align:'start'——锚即「可见首条」，对齐回视口顶 = 原位还原；
     - activeArticleId 的定位 effect 保持既有不变语义（选中文章仍由它负责），
       本 effect 只管滚动位置，两者互补不冲突；
     - 画廊布局不虚拟化（scrollToIndex 无效），回位回落现状。 */
  const positionRestoreNonce = useAppStore((s) => s.positionRestoreNonce);
  useEffect(() => {
    if (positionRestoreNonce === 0) return; // 初值非信号
    if (activeContentLayout === 'image') return; // 画廊非虚拟化，回位回落现状
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
     恢复成功后 rearmTopAnchor 把活锚重锚到落点（理由见 timelineAnchor.ts：
     防节流窗口把活锚滞留在恢复前位置，用户随即再离开时存档过期锚）。 */
  const switchRestoreNonce = useAppStore((s) => s.switchRestoreNonce);
  useEffect(() => {
    if (switchRestoreNonce === 0) return; // 初值非信号
    if (activeContentLayout === 'image') return; // 画廊非虚拟化，恢复回落现状
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
     - 画廊布局（image）非虚拟化、无 roving 焦点基建 → 回落现状
       （与 TASK-111 回位、切换返回恢复同一回落口径）。 */
  const readerCloseNonce = useAppStore((s) => s.readerCloseNonce);
  useEffect(() => {
    if (readerCloseNonce === 0) return; // 初值非信号
    if (activeContentLayout === 'image') return; // 画廊回落现状
    const idx = readerFocusReturnIndex(lastActiveArticleIdRef.current, items);
    if (idx == null) return; // 列表已切换 / 无档案 → 归零回落
    focusCardAt(idx);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [readerCloseNonce]);

  /* 滚动到底部附近 → 按需加载下一批文章（分页，避免一次性全量拉取）。 */
  const handleScroll = () => {
    /* P3[F5]/fix-2：判定收口在 scrollAwayRead.isUserScrollEvent——只有「近期有
       真实输入且不在程序性滚动抑制窗口内」的 scroll 事件才算用户滚动。
       保留既有结论（||）：一次拖拽会产生多个 scroll 事件，标读 effect 只消费
       一次，后续事件不得把已置位的用户滚动结论冲掉。 */
    scrollDrivenRef.current = scrollDrivenRef.current || isUserScrollEvent({
      gestureSeen: userGestureRef.current,
      programmaticUntil: programmaticScrollUntilRef.current,
      now: performance.now(),
    });
    /* TASK-111②：顶条锚记录（节流收口在 timelineAnchor.recordTopAnchor）——
       以当前可见首条目 id + filterKey 记账，供后台刷新落地后回位。程序性滚动
       （J/K / 回位）也照记：那是用户此刻的阅读位置。画廊布局不记录
       （虚拟化禁用时 range 不代表真实视口，回位消费侧同样回落）。
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

  return (
    <section className="timeline-col">
      <div className="timeline-control-bar">
        <div className="control-bar-main-row">
          <h3 className="view-title-text">{base}</h3>
          <div className="filter-sort-group">
            {activeViewFilter !== 'unread' && (
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
        id="timelineContentScroll"
        ref={scrollRef}
        onScroll={handleScroll}
      >
        {items.length === 0 && (
          <div className="timeline-empty-state">
            {activeViewFilter === 'starred' ? '暂无收藏内容' : activeViewFilter === 'today' ? '今天暂无新内容' : '暂无匹配内容'}
          </div>
        )}

        {/* 画廊（瀑布流多列）：不虚拟化，保持全量渲染（图片数量通常较少） */}
        {activeContentLayout === 'image' && (
          <div className="gallery-masonry-grid">
            {items.map((img, idx) => (
              <div key={img.id} data-card-id={img.id}>
                <GalleryCard item={img} cardIndex={idx} tabbable={idx === tabbableIndex} onMoveFocus={moveCardFocus} />
              </div>
            ))}
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
                <div
                  key={vi.key}
                  data-index={vi.index}
                  ref={rowVirtualizer.measureElement}
                  className="timeline-virtual-item"
                  style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${vi.start}px)` }}
                >
                  {activeContentLayout === 'article' && <ArticleCard art={item} onSelect={selectArticle} cardIndex={vi.index} tabbable={vi.index === tabbableIndex} onMoveFocus={moveCardFocus} />}
                  {/* TASK-114 X2：Social/Notif 卡补 onSelect（Enter/Space=选中，与 ArticleCard 同语义） */}
                  {activeContentLayout === 'social' && <SocialCard item={item} cardIndex={vi.index} tabbable={vi.index === tabbableIndex} onSelect={selectArticle} onMoveFocus={moveCardFocus} />}
                  {activeContentLayout === 'podcast' && <PodcastCard item={item} cardIndex={vi.index} tabbable={vi.index === tabbableIndex} onMoveFocus={moveCardFocus} />}
                  {activeContentLayout === 'notification' && <NotifCard item={item} cardIndex={vi.index} tabbable={vi.index === tabbableIndex} onSelect={selectArticle} onMoveFocus={moveCardFocus} />}
                </div>
              );
            })}
          </div>
        )}

        {/* 分页加载指示 / 滚动哨兵：加载中显示动画；到底显示「已到底」；否则占位等待滚动。
            TASK-052：此前整块被 `items.length > 0` 挡住——列表为空时哨兵不渲染，
            滚动事件无从触发，「该范围的老文章永远够不到」。列表为空但**批次已满**
            （articlesExhausted=false）时同样渲染：空列表 + 未到底 = 还有数据待取。
            真正到底（空且已到底）时不渲染，避免「没有更多了」与「暂无匹配内容」重复。
            TASK-094（REQ-107）：容器不可滚动时「滚动加载更多」不可执行，改为可点击的
            「加载更多」按钮（原生 button：键盘可聚焦，Enter/空格原生触发）。 */}
        {sentinel !== 'hidden' && (
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
  const feedName = useAppStore((s) => s.feedIndex.get(art.feedId)?.feed.name ?? '');
  const selected = activeArticleId === art.id;

  return (
    <div
      className={`article-card ${art.isRead ? 'read' : ''} ${selected ? 'active-selected' : ''}`}
      onClick={() => onSelect(art.id)}
      role="button"
      data-card-index={cardIndex}
      tabIndex={tabbable ? 0 : -1}
      onKeyDown={(e) => {
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

const SocialCard = memo(function SocialCard({ item, onSelect, cardIndex, tabbable, onMoveFocus }: {
  item: ArticleEntry;
  onSelect: (id: string) => void;
  cardIndex: number;
  tabbable: boolean;
  onMoveFocus: (from: number, delta: number) => void;
}) {
  const toggleEntryFlag = useAppStore((s) => s.toggleEntryFlag);
  const showToast = useAppStore((s) => s.showToast);
  const openLightbox = useAppStore((s) => s.openLightbox);
  const binding = useAppStore((s) => s.feedIndex.get(item.feedId));
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
  /* 社交卡片正文直接渲染 body.content（真值源 bodyById）：进入视口附近才懒加载
     水合（避免几百张卡片同时 getArticle 卡顿） */
  const hydrateRef = useLazyHydrate(item.id);
  /* 派生值提取为局部变量：JSX 表达式内不放可选链（oxc 解析限制，且更易读） */
  const feedName = binding ? binding.feed.name : '';
  /* 三态：null=跟随 feed 配置，true=手动展开，false=手动收起 */
  const [transOverride, setTransOverride] = useState<boolean | null>(null);
  /* fix-8：跟随 auto 配置展开的前提加「有产物/在途/出错」——auto 开但尚无译文时
     按收起处理，不再渲染空壳译文块（自动生成本身只在 Reader 打开文章时触发，
     卡片挂载刻意不发起，防滚动 IPC 风暴）；手动点「翻译」仍会就地触发生成。 */
  const showTranslate = transOverride ?? autoAiBlockOpen(
    feedConfig.autoTranslate,
    !!body.translatedContent,
    !!translatingCard,
    !!translateError,
  );
  /* 自动收起：渲染后测高，超过 260px 视为长内容（收起至 6 行 + 展开按钮）；
     与通知卡不同，这里是 HTML（高度比字符数准确——图片/换行/引用都会撑高）。
     ResizeObserver 而非一次性测量：正文里的图片懒加载完成后高度才真正
     确定，一次性 useEffect 会把"短文本+多图"的卡片误判为短内容 */
  const textRef = useRef<HTMLDivElement>(null);
  const [isLong, setIsLong] = useState(false);
  const [expanded, setExpanded] = useState(false);
  /* 记录上次 isLong 值：ResizeObserver 频繁触发，只在值真正翻转时才 setState，
     避免滚动/图片加载时每帧都重渲染（页面抖动的根因之一） */
  const isLongRef = useRef(false);
  useEffect(() => {
    const el = textRef.current;
    if (!el || typeof ResizeObserver === 'undefined') {
      if (el) setIsLong(el.scrollHeight > 260);
      return;
    }
    /* 折叠态下 scrollHeight 仍是完整内容高（overflow:hidden 不改变
       scrollHeight）——测量不受折叠影响 */
    const ro = new ResizeObserver(() => {
      const next = el.scrollHeight > 260;
      if (next !== isLongRef.current) {
        isLongRef.current = next;
        setIsLong(next);
      }
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [body.content]);

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
      ref={hydrateRef}
      className={`social-card ${item.isRead ? 'read' : ''}`}
      data-ctx="article"
      data-id={item.id}
      role="article"
      data-card-index={cardIndex}
      tabIndex={tabbable ? 0 : -1}
      onKeyDown={(e) => {
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
        {item.title && <div className="social-card-title">{item.title}</div>}
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
          className={`social-text ${isLong && !expanded ? 'collapsed' : ''}`}
          onClick={(e) => {
            const target = e.target as HTMLElement;
            if (target.tagName === 'IMG') {
              const src = (target as HTMLImageElement).currentSrc || (target as HTMLImageElement).src;
              if (src && !src.startsWith('data:')) {
                e.preventDefault();
                openLightbox(src);
              }
              return;
            }
            handleArticleLinkClick(e);
          }}
        >
          {body.content ? (
            <div dangerouslySetInnerHTML={{ __html: body.content }} />
          ) : body.state === 'failed' ? (
            <button
              className="hydrate-retry"
              onClick={() => useAppStore.getState().retryHydration(item.id)}
            >
              正文加载失败：{body.message}（点击重试）
            </button>
          ) : body.state === 'missing' ? (
            <span className="hydrate-placeholder">{body.message}</span>
          ) : body.state === 'ready' || body.state === 'cleared' ? (
            /* TASK-100：占位透明度并入 .hydrate-placeholder 类（此前内联 opacity 两处） */
            <span className="hydrate-placeholder">暂无正文</span>
          ) : (
            <span className="hydrate-placeholder">加载正文…</span>
          )}
        </div>
        {isLong && (
          <button className="notif-expand-btn social-expand-btn" onClick={() => setExpanded(!expanded)}>
            {expanded ? '收起内容 ▲' : '展开更多 ▼'}
          </button>
        )}
        <div className={"social-translated-block" + (showTranslate ? " show" : "")}>
          {/* TASK-065 N8/N11：未消毒流式产物按纯文本渲染；消毒后与 Reader 同口径按 HTML 渲染。
              TASK-122：cleared 判别态呈现「已清空」（与「从未生成过」的空白区分） */}
          {rawTranslated ? (
            <span>{body.translatedContent}</span>
          ) : body.state === 'cleared' && !body.translatedContent ? (
            <span className="hydrate-placeholder">AI 缓存已清空，可重新生成</span>
          ) : (
            <span dangerouslySetInnerHTML={{ __html: body.translatedContent }} />
          )}
          {translatingCard ? <span>翻译中…</span> : null}
          {/* fix-5：翻译失败内联错误行 + 重试（此前失败只有 toast 一闪，半截译文无恢复入口） */}
          {translateError && !translatingCard ? (
            <div className="ai-error-row">
              <span className="ai-error-text" title={translateError}>翻译失败：{translateError}</span>
              <button className="ai-retry-btn" onClick={() => useAppStore.getState().translateEntry(item.id)}>重试</button>
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
                 重试按钮变成死路径（P2-3）。 */
              if (next && (translateError || !body.translatedContent)) {
                /* 无译文或上次失败：实际触发生成/重试（P1-7 空壳修复 + fix-5） */
                useAppStore.getState().translateEntry(item.id);
              } else if (body.translatedContent) {
                showToast(next ? '已显示正文翻译' : '已隐藏正文翻译');
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
              void openExternal(item.url).catch(() => showToast('打开失败'));
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

const GalleryCard = memo(function GalleryCard({ item, cardIndex, tabbable, onMoveFocus }: {
  item: ArticleEntry;
  cardIndex: number;
  tabbable: boolean;
  onMoveFocus: (from: number, delta: number) => void;
}) {
  const toggleEntryFlag = useAppStore((s) => s.toggleEntryFlag);
  const openLightbox = useAppStore((s) => s.openLightbox);
  const selectArticle = useAppStore((s) => s.selectArticle);
  const feedName = useAppStore((s) => s.feedIndex.get(item.feedId)?.feed.name ?? '');
  /* 封面图防盗链代理：需要代理的图床（如 doubanio.com）走后端 fetch_image 拿
     bytes 转 data: URL；不需要的图原样直连。空 imageUrl（源没给图）则 src 为空，
     由 CSS 的 object 兜底显示。 */
  const [proxiedSrc, setProxiedSrc] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    const src = item.imageUrl;
    if (!src) return; // 无图：proxiedSrc 保持初始 null，走「无图」占位分支
    void proxyImageUrl(src, item.url).then((dataUrl) => {
      if (!alive) return;
      setProxiedSrc(dataUrl); // data: URL 或 null（不需要代理/失败）
    });
    return () => { alive = false; };
  }, [item.imageUrl, item.url]);
  /* fix-9（自检 UI-P2-1）：直连再失败不再留浏览器破图——记入共享封面失败态
     （lib/coverImage 的 onCoverError，与另四处同源）并就地出 cover-fallback 占位，
     五个封面位的失败视觉语言统一。 */
  const [imgFailed, setImgFailed] = useState(false);
  /* 换图源（imageUrl 变化）时清失败态：渲染期对比上一渲染的派生调整
     （React 官方模式，避免 set-state-in-effect 警告） */
  const [prevImageUrl, setPrevImageUrl] = useState(item.imageUrl);
  if (prevImageUrl !== item.imageUrl) {
    setPrevImageUrl(item.imageUrl);
    setImgFailed(false);
  }
  const imgSrc = proxiedSrc ?? item.imageUrl;
  /* 打开灯箱 = 用户"看到"了这张图；画廊布局下无阅读器列，
     以灯箱打开作为已读触发点（与 markReadOnOpen 设置解耦——
     点开大图本身就是"阅读完成"，不标读会出现永远未读的幽灵项） */
  const openImage = () => {
    /* 灯箱用代理后的 data: URL（若有）：豆瓣等防盗链图，原始 URL 在灯箱里
       no-referrer 也会 418；代理成功则用 data: URL 放大。 */
    const lightboxSrc = proxiedSrc ?? item.imageUrl;
    if (lightboxSrc) openLightbox(lightboxSrc, item.id);
    if (!item.isRead) {
      useAppStore.getState().markEntriesReadBulk([item.id]);
    } else {
      selectArticle(item.id);
    }
  };
  const onImgError = () => {
    if (imgSrc) onCoverError(imgSrc);
    setImgFailed(true);
  };
  return (
    <div className={`gallery-card ${item.isRead ? 'read' : ''}`} data-ctx="article" data-id={item.id}>
      {imgSrc && !imgFailed ? (
        <img
          src={imgSrc}
          loading="lazy"
          onClick={openImage}
          onError={onImgError}
          role="button"
          data-card-index={cardIndex}
          tabIndex={tabbable ? 0 : -1}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); openImage(); return; }
            if (e.key === 'ArrowRight' || e.key === 'ArrowDown') { e.preventDefault(); onMoveFocus(cardIndex, 1); }
            else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') { e.preventDefault(); onMoveFocus(cardIndex, -1); }
          }}
          alt={item.title}
          referrerPolicy="no-referrer"
        />
      ) : (
        <div
          className={`gallery-no-image${imgFailed ? ' cover-fallback' : ''}`}
          onClick={openImage}
          role="button"
          data-card-index={cardIndex}
          tabIndex={tabbable ? 0 : -1}
          data-cover-state={imgFailed ? 'failed' : 'empty'}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); openImage(); return; }
            if (e.key === 'ArrowRight' || e.key === 'ArrowDown') { e.preventDefault(); onMoveFocus(cardIndex, 1); }
            else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') { e.preventDefault(); onMoveFocus(cardIndex, -1); }
          }}
        >{imgFailed ? <Icons.image /> : '无图'}</div>
      )}
      <div className="gallery-meta">
        {/* TASK-100 U5：截断的画廊标题补 title（=未截断全文） */}
        <div className="gallery-title" title={item.title}>{item.title}</div>
        <div className="gallery-meta-row">
          <span>{feedName}</span>
          <div style={{ display: 'flex', gap: 6 }}>
            <button
              className={`toggle-action-btn notif-act ${item.isStarred ? 'act-on' : ''}`}
              onClick={(e) => { e.stopPropagation(); toggleEntryFlag(item.id, 'isStarred'); }}
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
  const feedName = useAppStore((s) => s.feedIndex.get(item.feedId)?.feed.name ?? '');
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
      className={`podcast-card ${item.isRead ? 'read' : ''}`}
      onClick={play}
      role="button"
      data-card-index={cardIndex}
      tabIndex={tabbable ? 0 : -1}
      onKeyDown={(e) => {
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
  const toggleEntryFlag = useAppStore((s) => s.toggleEntryFlag);
  const summarizeEntry = useAppStore((s) => s.summarizeEntry);
  const feedName = useAppStore((s) => s.feedIndex.get(item.feedId)?.feed.name ?? '');
  const summaryGenerating = useAppStore((s) => s.summarizingIds[item.id]);
  const summaryError = useAppStore((s) => s.summaryErrors[item.id] || '');
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
     尚无摘要时按收起处理，不再渲染空壳摘要框（自动生成只在 Reader 打开文章
     时触发，卡片挂载刻意不发起，防滚动 IPC 风暴）；手动点「摘要」仍会就地
     触发生成并展开。 */
  const summaryOpen = summaryOverride ?? autoAiBlockOpen(
    feedConfig.autoSummary,
    !!body.aiSummary,
    summaryGenerating,
    !!summaryError,
  );
  /* fix-8：译文块同口径（见上） */
  const transShow = transOverride ?? autoAiBlockOpen(
    feedConfig.autoTranslate,
    !!body.translatedContent,
    !!translatingCard,
    !!translateError,
  );
  /* 自动收起：按展示源文本判定（全文可得时按全文长度，否则按 snippet），
     短内容直接全文展示、不渲染展开按钮 */
  const isLong = (fullText || item.snippet || '').length > 120;

  return (
    /* TASK-100（UI P2-2 轻修）：同 SocialCard——补 role/tabIndex 融入 roving 体系。
       TASK-114 X2：Enter/Space=选中，e.target 守卫与 role 裁定同 SocialCard 注释。 */
    <div
      ref={hydrateRef}
      className={`notif-card ${item.isRead ? 'read' : ''}`}
      data-ctx="article"
      data-id={item.id}
      role="article"
      data-card-index={cardIndex}
      tabIndex={tabbable ? 0 : -1}
      onKeyDown={(e) => {
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
        <button
          className="hydrate-retry"
          onClick={() => useAppStore.getState().retryHydration(item.id)}
        >
          正文加载失败：{body.message}（点击重试）
        </button>
      ) : item.snippet ? (
        <div className={`notif-body-text ${isLong && !expanded ? 'collapsed' : ''}`}>{displayText}</div>
      ) : body.state === 'missing' ? (
        <div className="notif-body-text"><span className="hydrate-placeholder">{body.message}</span></div>
      ) : body.state === 'ready' || body.state === 'cleared' ? (
        <div className="notif-body-text"><span className="hydrate-placeholder">暂无正文</span></div>
      ) : (
        <div className="notif-body-text"><span className="hydrate-placeholder">加载正文…</span></div>
      )}

      <div className={`notif-translated-block ${transShow ? 'show' : ''}`}>
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
