import { memo, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { scrollAwayRange } from './scrollAwayRead';
import { useVirtualizer } from '@tanstack/react-virtual';
import { useShallow } from 'zustand/react/shallow';
import {
  useAppStore,
  LAYOUT_NAMES,
  VIEW_NAMES,
  podcastClickAction,
  selectVisibleEntries,
  selectFeedConfig,
} from '../store';
import { Icons } from './icons';
import { formatRelativeTime, formatDuration } from '../lib/format';
import { openExternal, handleArticleLinkClick } from '../lib/external';
import { proxyImageUrl } from '../lib/imageProxy';
import type { ArticleEntry } from '../types';
import { useEnteringClass } from './useEnteringClass';
import { sentinelMode } from './timelineSentinel';

/* ============================================================
   Timeline —— 顶栏（标题/筛选/排序/全部已读）+ 五布局渲染器

   交互设计：
   - 列表容器在布局/视图/筛选切换时做一次 160ms 的淡入过渡，
     避免内容瞬间替换造成的视觉跳动（"闪一下"）。
   - 列表切换后滚动位置归零（新列表从顶部阅读）。

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
  useEffect(() => {
    if (!activeArticleId) return;
    const idx = items.findIndex((a) => a.id === activeArticleId);
    if (idx >= 0) setFocusIndex(idx);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeArticleId]);

  /* 方向键在卡片间移动：目标可能未被虚拟化渲染，先 scrollToIndex 再于下一帧聚焦 */
  const moveCardFocus = (from: number, delta: number) => {
    const next = from + delta;
    if (next < 0 || next >= items.length) return;
    setFocusIndex(next);
    rowVirtualizer.scrollToIndex(next, { align: 'auto' });
    requestAnimationFrame(() => {
      const root = scrollRef.current;
      const el = root?.querySelector<HTMLElement>(`[data-card-index="${next}"]`);
      el?.focus();
    });
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
    document.getElementById('timelineContentScroll')?.scrollTo({ top: 0 });
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
     (1) 筛选上下文未变（换序列时不判滚出；下面的 effect 会同步重置基准）；
     (2) 本次 startIndex 变化确由**用户滚动**引起（而非筛选切换引发的程序性归零）。 */
  const scrollDrivenRef = useRef(false);
  /* 筛选上下文变化 → 重置基准并关闭本帧的滚出判定。
     与下面的归零 effect 同依赖，按声明顺序先执行 ⇒ 基准与本帧判定都已就绪，
     不依赖「归零 effect 先跑完」这一时序假设。 */
  useLayoutEffect(() => {
    lastStartIndexRef.current = 0;
    scrollDrivenRef.current = false;
  }, [filterKey]);
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
    rowVirtualizer.scrollToIndex(idx, { align: 'auto' });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeArticleId]);

  /* 滚动到底部附近 → 按需加载下一批文章（分页，避免一次性全量拉取）。 */
  const handleScroll = () => {
    /* P3[F5]：标记本次 range 变化源自用户滚动，供上面的「滚出上方」判定使用。
       scrollToIndex（J/K 定位）与筛选切换的归零都不经过本 handler，故不会被误判。 */
    scrollDrivenRef.current = true;
    const el = scrollRef.current;
    if (!el) return;
    // 距底部 600px 内视为"到底"，提前预加载，滚动体验更顺滑
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 600) {
      void loadMoreArticles();
    }
  };

  /* TASK-052 空列表补拉：列表为空且未到底（首批满页但当前范围/视图筛掉了全部条目，
     例如单源视图下首 500 条里没有该源的文章）时，容器不可滚动 ⇒ onScroll 永不触发
     ⇒ 分页永远停在第 1 页。这里在挂载与筛选口径变化后主动补拉一次。
     loadMoreArticles 自带入口守卫（在途/已到底直接返回），挂载期重复调用是安全的；
     若该范围真的没有更多数据，拉回空页后会置 articlesExhausted，本 effect 自然收敛。 */
  useEffect(() => {
    if (items.length > 0 || articlesExhausted) return;
    void loadMoreArticles();
  }, [filterKey, items.length, articlesExhausted, loadMoreArticles]);

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
                  {activeContentLayout === 'social' && <SocialCard item={item} />}
                  {activeContentLayout === 'podcast' && <PodcastCard item={item} cardIndex={vi.index} tabbable={vi.index === tabbableIndex} onMoveFocus={moveCardFocus} />}
                  {activeContentLayout === 'notification' && <NotifCard item={item} />}
                </div>
              );
            })}
          </div>
        )}

        {/* 分页加载指示 / 滚动哨兵：加载中显示动画；到底显示「已到底」；否则占位等待滚动。
            TASK-052：此前整块被 `items.length > 0` 挡住——列表为空时哨兵不渲染，
            滚动事件无从触发，「该范围的老文章永远够不到」。列表为空但**批次已满**
            （articlesExhausted=false）时同样渲染：空列表 + 未到底 = 还有数据待取。
            真正到底（空且已到底）时不渲染，避免「没有更多了」与「暂无匹配内容」重复。 */}
        {sentinel !== 'hidden' && (
          <div className="timeline-load-more">
            {sentinel === 'loading' ? (
              <span className="load-more-spinner" aria-label="加载中" />
            ) : sentinel === 'end' ? (
              <span className="load-more-end">没有更多了</span>
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
    内挂载的几十张卡片合并成一次 IPC，避免逐篇洪峰。 */
function useLazyHydrate(id: string): React.RefObject<HTMLDivElement | null> {
  const ref = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    /* 挂载即水合（幂等：已有正文则短路） */
    useAppStore.getState().ensureArticleContent(id);
  }, [id]);
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
          <h4 className="card-title">{art.title}</h4>
          <p className="card-snippet">{art.snippet}</p>
        </div>
        {art.cover && <img src={art.cover} className="card-cover-thumb" alt="cover" loading="lazy" referrerPolicy="no-referrer" />}
      </div>
      <div className="card-footer">
        <span>{art.author}</span>
        <span>{art.isStarred ? '★ 已收藏' : ''}</span>
      </div>
    </div>
  );
});

/* ---------- 社交卡片 ---------- */

const SocialCard = memo(function SocialCard({ item }: { item: ArticleEntry }) {
  const toggleEntryFlag = useAppStore((s) => s.toggleEntryFlag);
  const showToast = useAppStore((s) => s.showToast);
  const openLightbox = useAppStore((s) => s.openLightbox);
  const binding = useAppStore((s) => s.feedIndex.get(item.feedId));
  const feedConfig = useAppStore(useShallow((s) => selectFeedConfig(s, item.feedId)));
  /* 正文水合状态：错误态显示内联重试；空正文终态显示「暂无正文」而非永挂「加载正文…」 */
  const hydrationError = useAppStore((s) => s.hydrationErrors[item.id]);
  const hydrated = useAppStore((s) => s.hydratedIds[item.id]);
  /* 卡片级翻译状态（按 id 订阅，生成中指示） */
  const translatingCard = useAppStore((s) => s.translatingIds[item.id]);
  /* TASK-065 N11：译文当前是否为未消毒流式产物（决定纯文本/HTML 渲染路径） */
  const rawTranslated = useAppStore((s) => s.rawTranslatedIds[item.id]);
  /* 社交卡片正文直接渲染 item.content：进入视口附近才懒加载水合（避免几百张
     卡片同时 getArticle 卡顿） */
  const hydrateRef = useLazyHydrate(item.id);
  /* 派生值提取为局部变量：JSX 表达式内不放可选链（oxc 解析限制，且更易读） */
  const feedName = binding ? binding.feed.name : '';
  /* 三态：null=跟随 feed 配置，true=手动展开，false=手动收起 */
  const [transOverride, setTransOverride] = useState<boolean | null>(null);
  const showTranslate = transOverride ?? feedConfig.autoTranslate;
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
  }, [item.content]);

  return (
    <div ref={hydrateRef} className={`social-card ${item.isRead ? 'read' : ''}`} data-ctx="article" data-id={item.id}>
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
            <img> 点击走灯箱放大（与 Reader 一致），<a> 走外链 */}
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
          {item.content ? (
            <div dangerouslySetInnerHTML={{ __html: item.content }} />
          ) : hydrationError ? (
            <button
              className="hydrate-retry"
              onClick={() => useAppStore.getState().retryHydration(item.id)}
            >
              正文加载失败：{hydrationError}（点击重试）
            </button>
          ) : hydrated ? (
            <span className="hydrate-placeholder" style={{ opacity: 0.45 }}>暂无正文</span>
          ) : (
            <span className="hydrate-placeholder" style={{ opacity: 0.45 }}>加载正文…</span>
          )}
        </div>
        {isLong && (
          <button className="notif-expand-btn social-expand-btn" onClick={() => setExpanded(!expanded)}>
            {expanded ? '收起内容 ▲' : '展开更多 ▼'}
          </button>
        )}
        <div className={"social-translated-block" + (showTranslate ? " show" : "")}>
          {/* TASK-065 N8/N11：未消毒流式产物按纯文本渲染；消毒后与 Reader 同口径按 HTML 渲染 */}
          {rawTranslated ? (
            <span>{item.translatedContent}</span>
          ) : (
            <span dangerouslySetInnerHTML={{ __html: item.translatedContent }} />
          )}
          {translatingCard ? <span>翻译中…</span> : null}
        </div>
        <div className="social-actions-bar">
          <button
            className={`social-act-item ${item.isStarred ? 'starred' : ''}`}
            onClick={() => {
              toggleEntryFlag(item.id, 'isStarred');
              showToast(item.isStarred ? '已取消收藏' : '已收藏');
            }}
          >
            <Icons.star />
            <span>{item.isStarred ? '取消收藏' : '收藏'}</span>
          </button>
          <button
            className={`social-act-item ${item.isRead ? 'act-on' : ''}`}
            onClick={() => {
              toggleEntryFlag(item.id, 'isRead');
              showToast(item.isRead ? '已标为未读' : '已标为已读');
            }}
          >
            <Icons.check />
            <span>{item.isRead ? '标为未读' : '标为已读'}</span>
          </button>
          <button
            className={`social-act-item ${showTranslate ? 'active-translate' : ''}`}
            onClick={() => {
              const next = !showTranslate;
              if (next && !item.translatedContent) {
                /* 无译文：实际触发生成（P1-7 空壳修复） */
                useAppStore.getState().translateEntry(item.id);
              } else if (item.translatedContent) {
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
  const imgSrc = proxiedSrc ?? item.imageUrl;
  /* 打开灯箱 = 用户"看到"了这张图；画廊布局下无阅读器列，
     以灯箱打开作为已读触发点（与 markReadOnOpen 设置解耦——
     点开大图本身就是"阅读完成"，不标读会出现永远未读的幽灵项） */
  const openImage = () => {
    /* 灯箱用代理后的 data: URL（若有）：豆瓣等防盗链图，原始 URL 在灯箱里
       no-referrer 也会 418；代理成功则用 data: URL 放大。 */
    const lightboxSrc = proxiedSrc ?? item.imageUrl;
    if (lightboxSrc) openLightbox(lightboxSrc);
    if (!item.isRead) {
      useAppStore.getState().markEntriesReadBulk([item.id]);
    } else {
      selectArticle(item.id);
    }
  };
  return (
    <div className={`gallery-card ${item.isRead ? 'read' : ''}`} data-ctx="article" data-id={item.id}>
      {imgSrc ? (
        <img
          src={imgSrc}
          loading="lazy"
          onClick={openImage}
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
          className="gallery-no-image"
          onClick={openImage}
          role="button"
          data-card-index={cardIndex}
          tabIndex={tabbable ? 0 : -1}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); openImage(); return; }
            if (e.key === 'ArrowRight' || e.key === 'ArrowDown') { e.preventDefault(); onMoveFocus(cardIndex, 1); }
            else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') { e.preventDefault(); onMoveFocus(cardIndex, -1); }
          }}
        >无图</div>
      )}
      <div className="gallery-meta">
        <div className="gallery-title">{item.title}</div>
        <div className="gallery-meta-row">
          <span>{feedName}</span>
          <div style={{ display: 'flex', gap: 6 }}>
            <button
              className={`toggle-action-btn notif-act ${item.isStarred ? 'act-on' : ''}`}
              onClick={(e) => { e.stopPropagation(); toggleEntryFlag(item.id, 'isStarred'); }}
              title={item.isStarred ? '取消收藏' : '收藏'}
            >
              <span style={{ color: item.isStarred ? 'var(--star-color)' : 'inherit' }}>{item.isStarred ? '★' : '☆'}</span>
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
      <img src={item.cover} className="podcast-cover-box" alt="cover" loading="lazy" referrerPolicy="no-referrer" />
      <div style={{ flex: 1 }}>
        <div className="podcast-show-name">
          {feedName}
          {item.durationSec != null && ` · ${formatDuration(item.durationSec)}`}
        </div>
        <div className="podcast-title">{item.title}</div>
        <div className="podcast-desc">{item.snippet}</div>
      </div>
      <div className="podcast-play-circle">
        <Icons.play />
      </div>
    </div>
  );
});

/* ---------- 通知卡片 ---------- */

const NotifCard = memo(function NotifCard({ item }: { item: ArticleEntry }) {
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
  /* 进入视口附近才水合全文（与社交卡一致）：列表快照的 snippet 是 280 字截断，
     「展开更多」必须展示全文而非同一段截断文本 */
  const hydrateRef = useLazyHydrate(item.id);
  /* 展示文本：展开态优先水合全文（剥 HTML 标签），未水合/收起态用 snippet */
  const fullText = item.content
    ? item.content.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim()
    : '';
  const displayText = expanded && fullText ? fullText : item.snippet;
  /* 失败后卡片保持展开（展示错误 + 重试按钮） */
  const summaryOpen = summaryOverride ?? (feedConfig.autoSummary || !!summaryError);
  const transShow = transOverride ?? feedConfig.autoTranslate;
  /* 自动收起：按展示源文本判定（全文可得时按全文长度，否则按 snippet），
     短内容直接全文展示、不渲染展开按钮 */
  const isLong = (fullText || item.snippet || '').length > 120;

  return (
    <div ref={hydrateRef} className={`notif-card ${item.isRead ? 'read' : ''}`} data-ctx="article" data-id={item.id}>
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
              if (next && !item.translatedContent) {
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
            <span className="ai-error-text" title={summaryError}>生成失败：{summaryError}</span>
            <button className="ai-retry-btn" onClick={() => summarizeEntry(item.id)}>重试</button>
          </div>
        ) : summaryGenerating && !item.aiSummary ? (
          <div className="notif-ai-text ai-generating-hint">正在生成摘要…</div>
        ) : (
          <div className="notif-ai-text">{item.aiSummary}</div>
        )}
      </div>

      <div className={`notif-body-text ${isLong && !expanded ? 'collapsed' : ''}`}>{displayText}</div>

      <div className={`notif-translated-block ${transShow ? 'show' : ''}`}>
        {/* TASK-065 N8/N11：同 SocialCard——未消毒按纯文本，消毒后按 HTML */}
        {rawTranslated ? (
          <span>{item.translatedContent}</span>
        ) : (
          <span dangerouslySetInnerHTML={{ __html: item.translatedContent }} />
        )}
        {translatingCard ? <span>翻译中…</span> : null}
      </div>

      {isLong && (
        <button className="notif-expand-btn" onClick={() => setExpanded(!expanded)}>
          {expanded ? '收起内容 ▲' : '展开更多 ▼'}
        </button>
      )}
    </div>
  );
});
