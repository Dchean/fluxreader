import { useEffect, useRef, useState } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { useAppStore, selectFeedConfig } from '../store';
import { Icons } from './icons';
import { formatRelativeTime } from '../lib/format';
import { openExternal, handleArticleLinkClick } from '../lib/external';

/* ============================================================
   Reader —— 右侧沉浸阅读器
   正文结构：源Badge → 标题 → 作者/时间 → 工具栏 → AI摘要卡 → 正文
   ============================================================ */

export function Reader() {
  const isShowingTranslatedProse = useAppStore((s) => s.isShowingTranslatedProse);
  const isRawRenderMode = useAppStore((s) => s.isRawRenderMode);
  const summaryGenerating = useAppStore((s) => s.summaryGenerating);
  const settings = useAppStore((s) => s.settings);

  const toggleCurrentReadStatus = useAppStore((s) => s.toggleCurrentReadStatus);
  const toggleCurrentStar = useAppStore((s) => s.toggleCurrentStar);
  const toggleReaderRenderMode = useAppStore((s) => s.toggleReaderRenderMode);
  const toggleReaderTranslation = useAppStore((s) => s.toggleReaderTranslation);
  const triggerReaderSummary = useAppStore((s) => s.triggerReaderSummary);
  const extractCurrentArticle = useAppStore((s) => s.extractCurrentArticle);
  const dataMode = useAppStore((s) => s.dataMode);
  const showToast = useAppStore((s) => s.showToast);

  /* find() 返回既有元素引用（稳定）；config 返回新对象 → useShallow */
  const art = useAppStore((s) =>
    s.activeArticleId ? s.entries.find((a) => a.id === s.activeArticleId) ?? null : null,
  );
  const feedName = useAppStore((s) => (s.activeArticleId ? s.feedIndex.get(s.entries.find((a) => a.id === s.activeArticleId)?.feedId ?? '')?.feed.name ?? '' : ''));
  const config = useAppStore(
    useShallow((s) => selectFeedConfig(s, art?.feedId ?? '')),
  );
  /* 摘要卡显隐：默认跟随源/分类的自动摘要开关（未开启则不显示卡片）；
     手动点击「摘要」按钮覆写——点开即展开并生成，再点收起 */
  const [summaryOverride, setSummaryOverride] = useState<boolean | null>(null);
  const summaryOpen = summaryOverride ?? (config.autoSummary || summaryGenerating);

  /* 阅读时间估算（中文 ~400字/分钟，英文 ~220词/分钟） */
  const readTime = art
    ? `${Math.max(1, Math.round(art.content.replace(/<[^>]+>/g, '').length / 400))} 分钟阅读`
    : '';

  /* ---------- 滚动行为 ---------- */
  const scrollRef = useRef<HTMLDivElement>(null);

  /* 切换文章 → 滚动归零（否则上一篇的滚动深度带到下一篇）；手动摘要覆写态不跨文章保留 */
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 });
    setSummaryOverride(null);
  }, [art?.id]);

  /* 源级 autoSummary/autoTranslate：打开文章即自动触发。
     等内容水合完成后再触发（翻译需要正文）；已有缓存时后端会短路；
     静默失败：未配置 AI 不弹 toast（手动按钮仍会提示）。 */
  const hydrated = !!art?.content;
  useEffect(() => {
    if (!art || !hydrated) return;
    const st = useAppStore.getState();
    if (st.dataMode !== 'tauri') return;
    const cfg = selectFeedConfig(st, art.feedId);
    if (cfg.autoSummary && !art.aiSummary) st.triggerReaderSummary({ silent: true });
    if (cfg.autoTranslate && !art.translatedContent && !st.isShowingTranslatedProse) {
      st.toggleReaderTranslation({ silent: true });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [art?.id, hydrated]);

  /* 滚动到正文底部 → 标已读（markReadOnScrollBottom，实施方案 §3 已读行为②） */
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || !art || !settings.markReadOnScrollBottom) return;
    const articleId = art.id;
    const onScroll = () => {
      if (el.scrollTop + el.clientHeight >= el.scrollHeight - 24) {
        if (useAppStore.getState().activeArticleId === articleId) {
          useAppStore.getState().markEntriesReadBulk([articleId]);
        }
      }
    };
    el.addEventListener('scroll', onScroll, { passive: true });
    return () => el.removeEventListener('scroll', onScroll);
  }, [art?.id, settings.markReadOnScrollBottom, art?.content, isRawRenderMode, isShowingTranslatedProse]);

  return (
    <main className="reader-col" id="readerContainerCol">
      {!art && (
        <div className="reader-empty-state">
          <div style={{ fontSize: 32, marginBottom: 12 }}>📖</div>
          <h4 className="reader-empty-title">未选择文章</h4>
          <p className="reader-empty-desc">
            从列表中点击卡片即可在右侧载入正文并激活 AI 辅助阅读。
          </p>
        </div>
      )}

      {art && (
        <div className="reader-active-view visible">
          <div className="reader-scroll-content" ref={scrollRef} style={{ maxWidth: settings.maxWidth }}>
            <span className="reader-feed-badge">{feedName}</span>
            <h1 className="reader-article-title">{art.title}</h1>

            <div className="reader-byline">
              {art.author && <span>By {art.author}</span>}
              {art.author && <span>·</span>}
              <span>{formatRelativeTime(art.publishedAt)}</span>
              {art.tags.length > 0 && (
                <>
                  <span>·</span>
                  <span>{art.tags.join(' / ')}</span>
                </>
              )}
              {settings.showReadTime && (
                <>
                  <span>·</span>
                  <span>{readTime}</span>
                </>
              )}
            </div>

            {/* 操作工具栏 */}
            <div className="reader-actions-toolbar">
              <div className="reader-actions-left">
                <button className="toggle-action-btn" onClick={toggleCurrentReadStatus}>
                  {art.isRead ? <Icons.unreadDot /> : <Icons.check />}
                  <span>{art.isRead ? '标为未读' : '标为已读'}</span>
                </button>
                <button className="toggle-action-btn" onClick={toggleCurrentStar}>
                  {art.isStarred ? <Icons.starFilled /> : <Icons.star />}
                  <span>{art.isStarred ? '已收藏' : '收藏'}</span>
                </button>
                <button className="toggle-action-btn" onClick={toggleReaderRenderMode}>
                  {isRawRenderMode ? <Icons.doc /> : <Icons.code />}
                  <span>{isRawRenderMode ? '原文' : '渲染'}</span>
                </button>
                {dataMode === 'tauri' && (
                  <button
                    className={`toggle-action-btn ${art.fulltextExtracted ? 'active-accent' : ''}`}
                    onClick={extractCurrentArticle}
                    title={art.fulltextExtracted ? '已提取全文，点击刷新' : '从原文网页提取全文（Readability）'}
                  >
                    <Icons.doc />
                    <span>{art.fulltextExtracted ? '已全文' : '全文'}</span>
                  </button>
                )}
                <button
                  className="toggle-action-btn"
                  onClick={() => {
                    if (!art.url) { showToast('该条目没有原文网页地址'); return; }
                    void openExternal(art.url).catch(() => showToast('打开失败'));
                  }}
                >
                  <Icons.externalLink />
                  <span>源网页</span>
                </button>
              </div>
              <div className="reader-actions-right">
                <button
                  className={`toggle-action-btn ${summaryOpen ? 'active-accent' : ''}`}
                  onClick={() => {
                    /* 未开自动摘要的源：点开即展开卡片并触发生成（有缓存直接展示）；再点收起 */
                    if (!summaryOpen) triggerReaderSummary();
                    setSummaryOverride(!summaryOpen);
                  }}
                >
                  <Icons.spark />
                  <span>摘要</span>
                </button>
                <button className="toggle-action-btn" onClick={() => toggleReaderTranslation()}>
                  <Icons.globe />
                  <span>{isShowingTranslatedProse ? '显示原文' : '翻译'}</span>
                </button>
              </div>
            </div>

            {/* AI 摘要卡片 */}
            <div className={`ai-reader-box ${summaryOpen ? 'open' : ''}`}>
              <div className="ai-box-head">
                <div className="ai-badge-label">
                  <Icons.spark />
                  <span>摘要</span>
                </div>
              </div>
              <div className="ai-body-content">
                {summaryGenerating ? (
                  <span className="ai-generating-hint">⏳ 正在根据提示词生成摘要...</span>
                ) : (
                  <p>{art.aiSummary}</p>
                )}
              </div>
            </div>

            {/* 正文 */}
            <div
              className={`article-prose ${isRawRenderMode ? 'raw-render-mode' : ''}`}
              style={{
                fontFamily: settings.fontFamily,
                fontSize: settings.fontSize,
                lineHeight: settings.lineHeight / 100,
              }}
              onClick={handleArticleLinkClick}
              dangerouslySetInnerHTML={{
                __html: isRawRenderMode
                  ? art.rawContent
                  : isShowingTranslatedProse
                    ? art.translatedContent
                    : art.content,
              }}
            />
          </div>
        </div>
      )}
    </main>
  );
}
