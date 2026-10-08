import { useEffect, useRef, useState } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { useAppStore, selectArticleBody, selectFeedConfig } from '../store';
import { Icons } from './icons';
import { formatRelativeTime } from '../lib/format';
import { openExternal, handleArticleLinkClick } from '../lib/external';
import { proxyImagesInHtml } from '../lib/imageProxy';
import { useEnteringClass } from './useEnteringClass';
import { ReaderProse } from './ReaderProse';

/* ============================================================
   Reader —— 右侧沉浸阅读器
   正文结构：顶栏(状态操作) → 源Badge → 标题 → 作者/时间 → 阅读功能工具栏 → AI摘要卡 → 正文
   ============================================================ */

export function Reader() {
  const isShowingTranslatedProse = useAppStore((s) => s.isShowingTranslatedProse);
  /* TASK-065 N11：该文章的译文当前是否为未消毒流式产物（决定渲染路径） */
  const rawStream = useAppStore((s) => (s.activeArticleId ? s.rawTranslatedIds[s.activeArticleId] : undefined));
  const isRawRenderMode = useAppStore((s) => s.isRawRenderMode);
  const showFulltext = useAppStore((s) => s.showFulltext);
  /* F4：按当前文章 id 判定生成态，避免别的文章在生成时本文章误显「生成中」 */
  const summaryGenerating = useAppStore(
    (s) => (s.activeArticleId ? s.summarizingIds[s.activeArticleId] === true : false),
  );
  const translating = useAppStore((s) => s.translating);
  const settings = useAppStore((s) => s.settings);
  const openLightbox = useAppStore((s) => s.openLightbox);
  const playPodcastEpisode = useAppStore((s) => s.playPodcastEpisode);
  const player = useAppStore((s) => s.player);

  const toggleCurrentReadStatus = useAppStore((s) => s.toggleCurrentReadStatus);
  const toggleCurrentStar = useAppStore((s) => s.toggleCurrentStar);
  const toggleReaderRenderMode = useAppStore((s) => s.toggleReaderRenderMode);
  const toggleReaderFulltext = useAppStore((s) => s.toggleReaderFulltext);
  const toggleReaderTranslation = useAppStore((s) => s.toggleReaderTranslation);
  const triggerReaderSummary = useAppStore((s) => s.triggerReaderSummary);
  const dataMode = useAppStore((s) => s.dataMode);
  const showToast = useAppStore((s) => s.showToast);

  /* find() 返回既有元素引用（稳定）；config 返回新对象 → useShallow */
  const art = useAppStore((s) =>
    s.activeArticleId ? s.entries.find((a) => a.id === s.activeArticleId) ?? null : null,
  );
  /* TASK-122：正文/AI 产物读取单点——selectArticleBody（真值源 bodyById）。
     body 是快照对象（记录写入即换引用），nonce 依赖在 selector 内部建立。
     【为何必须包 useShallow】selectArticleBody 每次调用都经 bodyViewFrom(rec)
     返回**新对象**，zustand v5 把 selector 直接交给 useSyncExternalStore 且不做
     快照缓存 → React 每次 getSnapshot 都拿到新引用，判定为「未缓存」并反复重渲染
    （console.error "The result of getSnapshot should be cached..." → React #185
     Maximum update depth exceeded，打开文章即崩）。useShallow 逐字段浅比较，
     字段值不变即命中缓存引用，只有记录真变才触发渲染。同 Sidebar.tsx 的既有约定：
     返回新引用的派生 selector 必须包 useShallow。 */
  const body = useAppStore(useShallow((s) => selectArticleBody(s, s.activeArticleId)));
  /* 文章之间切换、以及原文↔译文视图切换时，正文淡入一次（REQ-005） */
  const readerViewRef = useRef<HTMLDivElement>(null);
  useEnteringClass(readerViewRef, `${art?.id ?? ''}|${isShowingTranslatedProse}`, 'reader-entering');
  /* 正文图片代理（防盗链）：对少数派等白名单式防盗链域名，走后端 fetch_image
     拿 bytes 转 data: URL 替换。代理目标 = 当前要显示的基础 HTML（全文或 RSS 原文），
     按 baseHtml 缓存，避免每次渲染重复抓图。 */
  const baseHtml = showFulltext ? body.content : body.rawContent;
  const [proxiedContent, setProxiedContent] = useState<{ key: string; html: string } | null>(null);
  useEffect(() => {
    if (!baseHtml) { setProxiedContent(null); return; }
    if (proxiedContent?.key === baseHtml) return; // 已代理过
    let alive = true;
    void proxyImagesInHtml(baseHtml, art?.url).then((result) => {
      if (!alive) return;
      if (result == null) { setProxiedContent(null); return; } // 无代理需要或浏览器环境
      setProxiedContent({ key: baseHtml, html: result });
    });
    return () => { alive = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [baseHtml, art?.url]);
  /* 正文两路呈现（审计 F14 / OPT-012）：源码态展示的是**原始文本**（当前用户选择的
     RSS 原文/提取全文/已消毒译文，未经图片代理——显示代理 base64 会冒充源码），
     渲染态保持现行为（代理命中时用代理产物）。流式未消毒译文两路都是纯文本。
     Note: 为何源码不能继续走 dangerouslySetInnerHTML、分离契约与取舍 —— 见
     .agents/notes/implemented/bug-fix/2026-10-08-阅读器源码显示契约.md */
  const renderHtml = isShowingTranslatedProse
    ? body.translatedContent
    : (proxiedContent?.key === baseHtml ? proxiedContent.html : baseHtml);
  const sourceText = isShowingTranslatedProse ? body.translatedContent : baseHtml;
  const feedName = useAppStore((s) => (s.activeArticleId ? s.feedIndex.get(s.entries.find((a) => a.id === s.activeArticleId)?.feedId ?? '')?.feed.name ?? '' : ''));
  const config = useAppStore(
    useShallow((s) => selectFeedConfig(s, art?.feedId ?? '')),
  );
  /* 内联错误：摘要/翻译失败按文章记录（空串 = 无错误） */
  const summaryError = useAppStore((s) => (art ? s.summaryErrors[art.id] || '' : ''));
  const translateError = useAppStore((s) => (art ? s.translateErrors[art.id] || '' : ''));
  /* 摘要卡显隐：默认跟随源/分类的自动摘要开关（未开启则不显示卡片）；
     手动点击「摘要」按钮覆写——点开即展开并生成，再点收起。
     失败后卡片保持展开（展示错误 + 重试按钮）。 */
  const [summaryOverride, setSummaryOverride] = useState<boolean | null>(null);
  const summaryOpen = summaryOverride ?? (config.autoSummary || summaryGenerating || !!summaryError);

  /* 阅读时间估算（中文 ~400字/分钟，英文 ~220词/分钟）。
     P2-5：正文未水合时按它算出来恒为「1 分钟阅读」，水合后又会跳到真实值
     （假数字 + 跳变）。未水合就不显示——宁可暂时没有这一项，也不显示一个
     确定错的数字。TASK-122：正文从 bodyById 读。 */
  const readTime = body.content
    ? `${Math.max(1, Math.round(body.content.replace(/<[^>]+>/g, '').length / 400))} 分钟阅读`
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
     静默失败：未配置 AI 不弹 toast（手动按钮仍会提示）。
     TASK-122：水合完成判定从 bodyById 状态取（ready；cleared = AI 被清理但
     正文保留——打开即按 auto 配置重新生成，即「清理后重开可再次生成」）。 */
  const hydrated = (body.state === 'ready' || body.state === 'cleared') && body.content !== '';
  useEffect(() => {
    if (!art || !hydrated) return;
    const st = useAppStore.getState();
    if (st.dataMode !== 'tauri') return;
    const cfg = selectFeedConfig(st, art.feedId);
    if (cfg.autoSummary && !body.aiSummary) st.triggerReaderSummary({ silent: true });
    if (cfg.autoTranslate && !body.translatedContent && !st.isShowingTranslatedProse) {
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
    // Note: exhaustive-deps — 依赖被有意收窄（只随目标变化执行一次），
    // 补全依赖会导致该副作用在无关状态变化时重复触发，故保留并显式标注。
  }, [art, settings.markReadOnScrollBottom, isRawRenderMode, isShowingTranslatedProse]);

  /* 正文点击代理：<a> 走外链（external.ts）；<img> 走灯箱放大；
     视频/音频原生控件点击不拦截 */
  const handleProseClick = (e: React.MouseEvent<HTMLDivElement>) => {
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
  };

  return (
    <main className="reader-col" id="readerContainerCol">
      {!art && (
        <div className="reader-empty-state">
          <div style={{ fontSize: 32, marginBottom: 12 }}>📖</div>
          <h4 className="reader-empty-title">未选择文章</h4>
          <p className="reader-empty-desc">
            在左侧选择一篇文章开始阅读。
          </p>
        </div>
      )}

      {art && (
        <div className="reader-active-view visible" ref={readerViewRef}>
          {/* 顶栏（不随正文滚动）：文章状态操作——标为已读/收藏/源网页/播放 */}
          <div className="reader-topbar">
            <button className="toggle-action-btn" onClick={toggleCurrentReadStatus} title={art.isRead ? '标为未读' : '标为已读'}>
              {art.isRead ? <Icons.unreadDot /> : <Icons.check />}
              <span>{art.isRead ? '标为未读' : '标为已读'}</span>
            </button>
            <button className="toggle-action-btn" onClick={toggleCurrentStar} title={art.isStarred ? '取消收藏' : '收藏'}>
              {art.isStarred ? <Icons.starFilled /> : <Icons.star />}
              <span>{art.isStarred ? '取消收藏' : '收藏'}</span>
            </button>
            <button
              className="toggle-action-btn"
              onClick={() => {
                if (!art.url) { showToast('该条目没有原文链接'); return; }
                void openExternal(art.url).catch(() => showToast('打开失败'));
              }}
              title="在浏览器中查看原文"
            >
              <Icons.externalLink />
              <span>查看原文</span>
            </button>
            {art.enclosureUrl && (
              <button
                className={`toggle-action-btn ${player.audioUrl === art.enclosureUrl ? 'active-accent' : ''}`}
                onClick={() => {
                  /* 播客/音频附件：阅读视图内直接进 PlayerBar（与播客卡片同一播放器） */
                  playPodcastEpisode(art.title, feedName, art.imageUrl ?? '', art.enclosureUrl ?? '', art.id);
                }}
                title={art.enclosureUrl}
              >
                <Icons.play />
                <span>播放</span>
              </button>
            )}
          </div>

          <div className="reader-scroll-content" ref={scrollRef} style={{ maxWidth: settings.maxWidth }} data-ctx="reader">
            <span className="reader-feed-badge">{feedName}</span>
            <h1 className="reader-article-title">{art.title}</h1>

              <div className="reader-byline">
                {/* TASK-100：署名中文化（此前 `By {author}` 中英混杂，其余 UI 全中文） */}
                {art.author && <span>作者：{art.author}</span>}
              {art.author && <span>·</span>}
              <span>{formatRelativeTime(art.publishedAt)}</span>
              {art.tags.length > 0 && (
                <>
                  <span>·</span>
                  <span>{art.tags.join(' / ')}</span>
                </>
              )}
              {settings.showReadTime && readTime && (
                <>
                  <span>·</span>
                  <span>{readTime}</span>
                </>
              )}
            </div>

            {/* 阅读功能工具栏（标题与正文之间）：摘要/翻译/全文/渲染 */}
            <div className="reader-actions-toolbar">
              <div className="reader-actions-left">
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
                {dataMode === 'tauri' && (
                  <button
                    className={`toggle-action-btn ${showFulltext ? 'active-accent' : ''}`}
                    onClick={toggleReaderFulltext}
                    title={
                      !body.fulltextExtracted
                        ? '从原文网页提取全文（Readability）'
                        : showFulltext
                          ? '切换到 RSS 原文'
                          : '切换到已提取的全文'
                    }
                  >
                    <Icons.doc />
                    <span>
                      {!body.fulltextExtracted ? '提取全文' : showFulltext ? 'RSS 原文' : '显示全文'}
                    </span>
                  </button>
                )}
                <button className="toggle-action-btn" onClick={toggleReaderRenderMode} title={isRawRenderMode ? '切换到 HTML 渲染' : '查看原始 HTML 源码'}>
                  {isRawRenderMode ? <Icons.doc /> : <Icons.code />}
                  <span>{isRawRenderMode ? '渲染' : '源码'}</span>
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
                  /* TASK-100：生成中文案统一「正在生成摘要…」（与通知卡一致） */
                  <span className="ai-generating-hint">正在生成摘要…</span>
                ) : summaryError ? (
                  <div className="ai-error-row">
                    <span className="ai-error-text" title={summaryError}>摘要生成失败：{summaryError}</span>
                    <button className="ai-retry-btn" onClick={() => triggerReaderSummary()}>重试</button>
                  </div>
                ) : body.state === 'cleared' && !body.aiSummary ? (
                  /* TASK-122：AI 缓存已被清理（cleared 判别态）——呈现「已清空」而非
                     与「从未生成过」混同的空白（'' vs cleared 语义区分） */
                  <span className="ai-generating-hint">AI 缓存已清空，可重新生成</span>
                ) : (
                  <p>{body.aiSummary}</p>
                )}
              </div>
            </div>

            {/* 翻译失败内联提示（正文上方，切换按钮旁） */}
            {translateError && !isShowingTranslatedProse && !translating && (
              <div className="ai-error-row reader-translate-error">
                <span className="ai-error-text" title={translateError}>翻译失败：{translateError}</span>
                <button className="ai-retry-btn" onClick={() => toggleReaderTranslation()}>重试</button>
              </div>
            )}

            {/* 正文：源码/渲染/流式三条路径收口在 ReaderProse（F14/OPT-012）。
                传参含义：renderHtml 走渲染态（含代理产物），sourceText 走源码态
                （原始文本，不经代理），isStreamingTranslation 走纯文本。 */}
            <ReaderProse
              renderHtml={renderHtml}
              sourceText={sourceText}
              isSourceMode={isRawRenderMode}
              isStreamingTranslation={isShowingTranslatedProse && !!rawStream}
              style={{
                fontFamily: settings.fontFamily,
                fontSize: settings.fontSize,
                lineHeight: settings.lineHeight / 100,
              }}
              onClick={handleProseClick}
            />
          </div>
        </div>
      )}
    </main>
  );
}
