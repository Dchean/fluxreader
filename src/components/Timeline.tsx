import { useEffect, useRef, useState } from 'react';
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

/** 滚动容器 ref 上挂的已注册卡片 Map（id → element） */
type RegisteredCards = Map<string, HTMLElement>;

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

  /* 返回新数组的派生 selector 必须包 useShallow */
  const items = useAppStore(useShallow(selectVisibleEntries));

  /* 筛选上下文变化 → CSS 动画通过 key 重放淡入；DOM key 同时驱动滚动归零 */
  const filterKey = `${activeContentLayout}|${activeViewFilter}|${activeFeedFilter}|${timelineFilter}|${timelineSort}`;

  useEffect(() => {
    document.getElementById('timelineContentScroll')?.scrollTo({ top: 0 });
  }, [filterKey]);

  /* ---------- 滚动出列表视口 → 标已读（markReadOnScrollOut） ---------- */
  const scrollRef = useRef<HTMLDivElement>(null);
  const cardRegistry = useRef<RegisteredCards>(new Map());

  useEffect(() => {
    const container = scrollRef.current;
    if (!container) return;
    if (!useAppStore.getState().settings.markReadOnScrollOut) return;
    if (items.length === 0) return;

    /* rootMargin 只留 10% 上沿裁剪带：卡片完全越过视口上沿才算"滚出"，
       避免刚挂载时视口内卡片被误标（IntersectionObserver 初次回调即报告相交状态） */
    const io = new IntersectionObserver(
      (records) => {
        const exitedIds: string[] = [];
        for (const rec of records) {
          const id = (rec.target as HTMLElement).dataset.cardId;
          if (!id) continue;
          const above = rec.boundingClientRect.top < container.getBoundingClientRect().top;
          if (!rec.isIntersecting && above) {
            exitedIds.push(id);
          }
        }
        if (exitedIds.length > 0) {
          useAppStore.getState().markEntriesReadBulk(exitedIds);
        }
      },
      { root: container, threshold: 0, rootMargin: '0px 0px -90% 0px' },
    );

    for (const el of cardRegistry.current.values()) io.observe(el);
    return () => io.disconnect();
  }, [filterKey, items.length, activeContentLayout]);

  /* 卡片挂载/卸载时维护注册表 */
  const registerCard = (id: string, el: HTMLElement | null) => {
    if (el) cardRegistry.current.set(id, el);
    else cardRegistry.current.delete(id);
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
        className="timeline-scroll-body list-entering"
        id="timelineContentScroll"
        key={filterKey}
        ref={scrollRef}
      >
        {items.length === 0 && (
          <div className="timeline-empty-state">
            {activeViewFilter === 'starred' ? '暂无收藏内容' : activeViewFilter === 'today' ? '今天暂无新内容' : '暂无匹配内容'}
          </div>
        )}
        {activeContentLayout === 'article' &&
          items.map((art) => (
            <div key={art.id} data-card-id={art.id} ref={(el) => registerCard(art.id, el)}>
              <ArticleCard art={art} onSelect={selectArticle} />
            </div>
          ))}
        {activeContentLayout === 'social' && (
          <div className="social-feed-wrap">
            {items.map((s) => <SocialCard key={s.id} item={s} />)}
          </div>
        )}
        {activeContentLayout === 'image' && (
          <div className="gallery-masonry-grid">
            {items.map((img) => (
              <div key={img.id} data-card-id={img.id} ref={(el) => registerCard(img.id, el)}>
                <GalleryCard item={img} />
              </div>
            ))}
          </div>
        )}
        {activeContentLayout === 'podcast' && (
          <div className="podcast-feed-wrap">
            {items.map((p) => (
              <div key={p.id} data-card-id={p.id} ref={(el) => registerCard(p.id, el)}>
                <PodcastCard item={p} />
              </div>
            ))}
          </div>
        )}
        {activeContentLayout === 'notification' && (
          <div className="notif-feed-wrap">
            {items.map((n) => (
              <div key={n.id} data-card-id={n.id} ref={(el) => registerCard(n.id, el)}>
                <NotifCard item={n} />
              </div>
            ))}
          </div>
        )}
      </div>
    </section>
  );
}

/* ---------- 文章卡片 ---------- */

function ArticleCard({ art, onSelect }: { art: ArticleEntry; onSelect: (id: string) => void }) {
  const activeArticleId = useAppStore((s) => s.activeArticleId);
  const feedName = useAppStore((s) => s.feedIndex.get(art.feedId)?.feed.name ?? '');
  const selected = activeArticleId === art.id;
  const cardRef = useRef<HTMLDivElement>(null);

  /* 从搜索/命令面板选中（范围切换后首次渲染）→ 滚动定位到该卡片 */
  useEffect(() => {
    if (selected) cardRef.current?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }, [selected, art.id]);

  return (
    <div
      ref={cardRef}
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
}

/* ---------- 社交卡片 ---------- */

function SocialCard({ item }: { item: ArticleEntry }) {
  const toggleEntryFlag = useAppStore((s) => s.toggleEntryFlag);
  const showToast = useAppStore((s) => s.showToast);
  const binding = useAppStore((s) => s.feedIndex.get(item.feedId));
  const feedConfig = useAppStore(useShallow((s) => selectFeedConfig(s, item.feedId)));
  /* 社交卡片正文直接渲染 item.content：挂载时懒加载水合（列表快照不含 HTML） */
  useEffect(() => {
    useAppStore.getState().ensureArticleContent(item.id);
  }, [item.id]);
  /* 派生值提取为局部变量：JSX 表达式内不放可选链（oxc 解析限制，且更易读） */
  const feedName = binding ? binding.feed.name : '';
  /* 三态：null=跟随 feed 配置，true=手动展开，false=手动收起 */
  const [transOverride, setTransOverride] = useState<boolean | null>(null);
  const showTranslate = transOverride ?? feedConfig.autoTranslate;
  /* 自动收起：渲染后测高，超过 260px 视为长内容（收起至 6 行 + 展开按钮）；
     与通知卡不同，这里是 HTML（高度比字符数准确——图片/换行/引用都会撑高） */
  const textRef = useRef<HTMLDivElement>(null);
  const [isLong, setIsLong] = useState(false);
  const [expanded, setExpanded] = useState(false);
  useEffect(() => {
    if (!textRef.current) return;
    setIsLong(textRef.current.scrollHeight > 260);
  }, [item.content]);

  return (
    <div className={`social-card ${item.isRead ? 'read' : ''}`}>
      <div className="social-avatar">{feedName.charAt(0) || '?'}</div>
      <div className="social-body">
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
            : <span style={{ opacity: 0.45 }}>加载正文…</span>}
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
            className="social-act-item"
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
}

/* ---------- 画廊卡片 ---------- */

function GalleryCard({ item }: { item: ArticleEntry }) {
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
}

/* ---------- 播客卡片 ---------- */

function PodcastCard({ item }: { item: ArticleEntry }) {
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
}

/* ---------- 通知卡片 ---------- */

function NotifCard({ item }: { item: ArticleEntry }) {
  const feedConfig = useAppStore(useShallow((s) => selectFeedConfig(s, item.feedId)));
  const toggleEntryFlag = useAppStore((s) => s.toggleEntryFlag);
  const summarizeEntry = useAppStore((s) => s.summarizeEntry);
  const feedName = useAppStore((s) => s.feedIndex.get(item.feedId)?.feed.name ?? '');
  const summaryGenerating = useAppStore((s) => s.summaryGenerating);
  const summaryError = useAppStore((s) => s.summaryErrors[item.id] || '');
  const [summaryOverride, setSummaryOverride] = useState<boolean | null>(null);
  const [transOverride, setTransOverride] = useState<boolean | null>(null);
  const [expanded, setExpanded] = useState(false);
  /* 失败后卡片保持展开（展示错误 + 重试按钮） */
  const summaryOpen = summaryOverride ?? (feedConfig.autoSummary || !!summaryError);
  const transShow = transOverride ?? feedConfig.autoTranslate;
  /* 自动收起：短内容（≤120 字符，折叠 2 行足够容纳）直接全文展示，
     不渲染展开按钮——只有真正超长的内容才收起 */
  const isLong = (item.snippet || '').length > 120;

  return (
    <div className={`notif-card ${item.isRead ? 'read' : ''}`}>
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

      <div className={`notif-body-text ${isLong && !expanded ? 'collapsed' : ''}`}>{item.snippet}</div>

      <div className={`notif-translated-block ${transShow ? 'show' : ''}`}>{item.translatedContent}</div>

      {isLong && (
        <button className="notif-expand-btn" onClick={() => setExpanded(!expanded)}>
          {expanded ? '收起内容 ▲' : '展开更多 ▼'}
        </button>
      )}
    </div>
  );
}
