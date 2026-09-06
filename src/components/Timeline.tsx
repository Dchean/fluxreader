import { memo, useEffect, useRef, useState } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { useShallow } from 'zustand/react/shallow';
import {
  useAppStore,
  LAYOUT_NAMES,
  VIEW_NAMES,
  selectVisibleEntries,
  selectFeedConfig,
} from '../store';
import { Icons } from './icons';
import { formatRelativeTime, formatDuration } from '../lib/format';
import { openExternal, handleArticleLinkClick } from '../lib/external';
import type { ArticleEntry } from '../types';

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
  useEffect(() => {
    if (!useAppStore.getState().settings.markReadOnScrollOut) return;
    if (timelineFilter !== 'unread') return;
    const start = rowVirtualizer.range?.startIndex ?? 0;
    if (start > lastStartIndexRef.current) {
      const exitedIds: string[] = [];
      for (let i = lastStartIndexRef.current; i < start && i < items.length; i++) {
        const it = items[i];
        if (it && !it.isRead) exitedIds.push(it.id);
      }
      if (exitedIds.length > 0) {
        useAppStore.getState().markEntriesReadBulk(exitedIds);
      }
    }
    lastStartIndexRef.current = start;
  }, [rowVirtualizer.range?.startIndex, items, timelineFilter]);

  /* 选中文章（搜索/命令面板/J/K 导航）→ 滚动定位到该卡片。虚拟化下卡片
     可能不在可视区（不渲染），不能用 scrollIntoView；改用 virtualizer 的
     scrollToIndex。
     持续定位：动态高度虚拟滚动下，长卡片/图片加载会改变行高，一次性
     scrollToIndex 在未测量时会定位不准。依赖 totalSize——列表总高度每变化
     就 re-check，直到目标稳定。align:'auto' 只在目标不完全可见时才滚动，
     点击可见卡片不会抖动（papr 同款方案）。 */
  const totalSize = rowVirtualizer.getTotalSize();
  useEffect(() => {
    if (!activeArticleId) return;
    const idx = items.findIndex((a) => a.id === activeArticleId);
    if (idx < 0) return; // 目标尚未加载进 items（如 anchorToArticle 异步中）——等 totalSize 变化重试
    rowVirtualizer.scrollToIndex(idx, { align: 'auto' });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeArticleId, items, totalSize]);

  /* 滚动到底部附近 → 按需加载下一批文章（分页，避免一次性全量拉取）。 */
  const handleScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    // 距底部 600px 内视为"到底"，提前预加载，滚动体验更顺滑
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 600) {
      void loadMoreArticles();
    }
  };

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
        </div>
        <div className="timeline-actions-row">
          <div className="filter-sort-group">
            {activeViewFilter !== 'unread' && (
              <button className="toggle-action-btn" onClick={toggleTimelineFilter}>
                <Icons.unreadDot />
                <span>显示: {timelineFilter === 'all' ? '全部' : '未读'}</span>
              </button>
            )}
            <button className="toggle-action-btn" onClick={toggleTimelineSort}>
              <Icons.sort />
              <span>排序: {timelineSort === 'newest' ? '最新 ↓' : '最早 ↑'}</span>
            </button>
            <button className="toggle-action-btn" onClick={markCurrentViewAllRead}>
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
            {items.map((img) => (
              <div key={img.id} data-card-id={img.id}>
                <GalleryCard item={img} />
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
                  {activeContentLayout === 'article' && <ArticleCard art={item} onSelect={selectArticle} />}
                  {activeContentLayout === 'social' && <SocialCard item={item} />}
                  {activeContentLayout === 'podcast' && <PodcastCard item={item} />}
                  {activeContentLayout === 'notification' && <NotifCard item={item} />}
                </div>
              );
            })}
          </div>
        )}

        {/* 分页加载指示：加载中显示动画；到底显示「已到底」；否则占位等待滚动 */}
        {items.length > 0 && (
          <div className="timeline-load-more">
            {articlesLoading ? (
              <span className="load-more-spinner" aria-label="加载中" />
            ) : articlesExhausted ? (
              <span className="load-more-end">— 已到底 —</span>
            ) : (
              <span className="load-more-idle">下拉加载更多</span>
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

const ArticleCard = memo(function ArticleCard({ art, onSelect }: { art: ArticleEntry; onSelect: (id: string) => void }) {
  const activeArticleId = useAppStore((s) => s.activeArticleId);
  const feedName = useAppStore((s) => s.feedIndex.get(art.feedId)?.feed.name ?? '');
  const selected = activeArticleId === art.id;

  return (
    <div
      className={`article-card ${art.isRead ? 'read' : ''} ${selected ? 'active-selected' : ''}`}
      onClick={() => onSelect(art.id)}
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
  const binding = useAppStore((s) => s.feedIndex.get(item.feedId));
  const feedConfig = useAppStore(useShallow((s) => selectFeedConfig(s, item.feedId)));
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
    <div ref={hydrateRef} className={`social-card ${item.isRead ? 'read' : ''}`}>
      <div className="social-avatar">{feedName.charAt(0) || '?'}</div>
      <div className="social-body">
        {/* 标题：社交布局此前漏显示——正文太长时一眼无法辨识内容主题 */}
        {item.title && <div className="social-card-title">{item.title}</div>}
        <div className="social-author-row">
          <strong className="social-author-name">{item.author}</strong>
          <span className="social-handle">{feedName}</span>
          <span className="social-date">{formatRelativeTime(item.publishedAt)}</span>
        </div>
        {/* 正文是消毒后的 HTML（同 Reader）；水合完成前显示轻量占位（毫秒级） */}
        <div
          ref={textRef}
          className={`social-text ${isLong && !expanded ? 'collapsed' : ''}`}
          onClick={handleArticleLinkClick}
        >
          {item.content
            ? <div dangerouslySetInnerHTML={{ __html: item.content }} />
            : <span className="hydrate-placeholder" style={{ opacity: 0.45 }}>加载正文…</span>}
        </div>
        {isLong && (
          <button className="notif-expand-btn social-expand-btn" onClick={() => setExpanded(!expanded)}>
            {expanded ? '收起内容 ▲' : '展开更多 ▼'}
          </button>
        )}
        <div className={"social-translated-block" + (showTranslate ? " show" : "")}>{item.translatedContent}</div>
        <div className="social-actions-bar">
          <button
            className={`social-act-item ${item.isStarred ? 'starred' : ''}`}
            onClick={() => {
              toggleEntryFlag(item.id, 'isStarred');
              showToast(item.isStarred ? '已取消收藏' : '已加入收藏');
            }}
          >
            <Icons.star />
            <span>{item.isStarred ? '已收藏' : '收藏'}</span>
          </button>
          <button
            className={`social-act-item ${item.isRead ? 'act-on' : ''}`}
            onClick={() => {
              toggleEntryFlag(item.id, 'isRead');
              showToast(item.isRead ? '已标记为未读' : '已标记为已读');
            }}
          >
            <Icons.check />
            <span>{item.isRead ? '标为未读' : '标为已读'}</span>
          </button>
          <button
            className={`social-act-item ${showTranslate ? 'active-translate' : ''}`}
            onClick={() => {
              const next = !showTranslate;
              setTransOverride(next);
              showToast(next ? '已显示正文翻译' : '已隐藏正文翻译');
            }}
          >
            <Icons.globe />
            <span>翻译</span>
          </button>
          <button
            className="social-act-item"
            onClick={() => {
              if (!item.url) { showToast('该条目没有原文网页地址'); return; }
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

const GalleryCard = memo(function GalleryCard({ item }: { item: ArticleEntry }) {
  const toggleEntryFlag = useAppStore((s) => s.toggleEntryFlag);
  const openLightbox = useAppStore((s) => s.openLightbox);
  const selectArticle = useAppStore((s) => s.selectArticle);
  const feedName = useAppStore((s) => s.feedIndex.get(item.feedId)?.feed.name ?? '');
  /* 打开灯箱 = 用户"看到"了这张图；画廊布局下无阅读器列，
     以灯箱打开作为已读触发点（与 markReadOnOpen 设置解耦——
     点开大图本身就是"阅读完成"，不标读会出现永远未读的幽灵项） */
  const openImage = () => {
    if (item.imageUrl) openLightbox(item.imageUrl);
    if (!item.isRead) {
      useAppStore.getState().markEntriesReadBulk([item.id]);
    } else {
      selectArticle(item.id);
    }
  };
  return (
    <div className={`gallery-card ${item.isRead ? 'read' : ''}`}>
      <img src={item.imageUrl} loading="lazy" onClick={openImage} alt={item.title} referrerPolicy="no-referrer" />
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
              <span>{item.isRead ? '已读' : '未读'}</span>
            </button>
          </div>
        </div>
      </div>
    </div>
  );
});

/* ---------- 播客卡片 ---------- */

const PodcastCard = memo(function PodcastCard({ item }: { item: ArticleEntry }) {
  const playPodcastEpisode = useAppStore((s) => s.playPodcastEpisode);
  const feedName = useAppStore((s) => s.feedIndex.get(item.feedId)?.feed.name ?? '');
  return (
    <div
      className={`podcast-card ${item.isRead ? 'read' : ''}`}
      onClick={() => playPodcastEpisode(item.title, feedName, item.cover ?? '', item.enclosureUrl ?? '', item.id)}
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
  const summaryGenerating = useAppStore((s) => s.summaryGenerating);
  const summaryError = useAppStore((s) => s.summaryErrors[item.id] || '');
  const [summaryOverride, setSummaryOverride] = useState<boolean | null>(null);
  const [transOverride, setTransOverride] = useState<boolean | null>(null);
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
    <div ref={hydrateRef} className={`notif-card ${item.isRead ? 'read' : ''}`}>
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
            onClick={() => setTransOverride(!transShow)}
          >
            <Icons.globe />
            <span>翻译</span>
          </button>
          <button
            className={`toggle-action-btn notif-act ${item.isRead ? 'act-on' : ''}`}
            onClick={() => toggleEntryFlag(item.id, 'isRead')}
          >
            <Icons.check />
            <span>{item.isRead ? '已读' : '标为已读'}</span>
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
          <div className="notif-ai-text ai-generating-hint">⏳ 正在生成摘要...</div>
        ) : (
          <div className="notif-ai-text">{item.aiSummary}</div>
        )}
      </div>

      <div className={`notif-body-text ${isLong && !expanded ? 'collapsed' : ''}`}>{displayText}</div>

      <div className={`notif-translated-block ${transShow ? 'show' : ''}`}>{item.translatedContent}</div>

      {isLong && (
        <button className="notif-expand-btn" onClick={() => setExpanded(!expanded)}>
          {expanded ? '收起内容 ▲' : '展开更多 ▼'}
        </button>
      )}
    </div>
  );
});
