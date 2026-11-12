import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useAppStore, CONTENT_LAYOUTS, LAYOUT_NAMES } from '../store';
import { api, articleRowToEntry } from '../lib/api';
import { Icons, LayoutIcon } from './icons';
import { ModalOverlay, FluxDropdown, SwitchInline, AiModeSelect } from './primitives';
import { useFocusTrap } from './useFocusTrap';
import { LAYOUT_NO_AI } from './settings/shared';
import { normalizeFeedAiMode } from '../types';
import { categoryAiDefaults } from '../store/aiPolicy';
import type { ArticleEntry, CategoryGroup, ContentLayoutType, FeedAiMode, FeedItem } from '../types';
import { anchorScopeNav } from './anchorScopeNav';
import { CoverImage } from './CoverImage';
import { galleryMediaKey } from '../lib/galleryMedia';
import { useImageResource } from './useImageResource';
import { canProxyImages, needsImageProxy } from '../lib/imageProxy';
import {
  addFeedShowsSyncSwitch,
  addFeedSyncHint,
  refreshAllLabel,
  searchErrorVisible,
} from '../lib/copyPolicy';

/* TASK-100：LAYOUT_NO_AI 双定义收敛——唯一定义在 settings/shared.ts，
   此处（与设置页）共用同一份，避免改一处漏一处。 */

/** 稳定空数组：搜索结果空查询分支引用（避免 useMemo 依赖每次渲染都变化） */
const EMPTY_RESULTS: ArticleEntry[] = [];

/* ============================================================
   全局搜索 / 灯箱 / 新建分类 / 添加订阅源 四个浮层
   ============================================================ */

const LAYOUT_OPTIONS = CONTENT_LAYOUTS.map((l) => {
  const Icon = LayoutIcon[l];
  return { value: l as string, label: LAYOUT_NAMES[l], icon: <Icon /> };
});

export function SearchModal() {
  const searchOpen = useAppStore((s) => s.searchOpen);
  const closeSearch = useAppStore((s) => s.closeSearch);

  return (
    <ModalOverlay open={searchOpen} onClose={closeSearch} contentWidth={580} ariaLabel="搜索与命令面板">
      <SearchModalBody key={String(searchOpen)} onClose={closeSearch} />
    </ModalOverlay>
  );
}

/* ============================================================
   命令面板（对齐 Papr CommandPalette 行为）：
   - 空查询即显示全部命令 + 订阅源（打开即可用，不是只有输入才有结果）
   - 输入过滤：命令按标签、订阅源按名称/URL、文章走后端 FTS5
   - IME 保护：Enter 判 isComposing（中文输入法选词回车不误触）
   - 键盘导航：↑↓ 循环 + Enter 执行；键盘移动时 scrollIntoView
     （鼠标 hover 不触发滚动，防止列表内容在光标下跳动）
   - 底部快捷键提示条
   ============================================================ */

interface PaletteItem {
  id: string;
  group: 'command' | 'feed' | 'article';
  label: string;
  hint: string;
  run: () => void;
}

function SearchModalBody({ onClose }: { onClose: () => void }) {
  const [q, setQ] = useState('');
  const [debounced, setDebounced] = useState('');
  const [results, setResults] = useState<ArticleEntry[]>([]);
  const [searching, setSearching] = useState(false);
  const [searchNonce, setSearchNonce] = useState(0);
  const [cursor, setCursor] = useState(0);
  const [searchError, setSearchError] = useState(false);
  const feedIndex = useAppStore((s) => s.feedIndex);
  const sourceIndex = useAppStore((s) => s.sourceIndex);
  const syncMode = useAppStore((s) => s.settings.syncMode);
  const listRef = useRef<HTMLDivElement>(null);
  /* 键盘移动标记：scrollIntoView 只在键盘导航时触发 */
  const keyboardNav = useRef(false);

  /* 文章搜索防抖（审计卡2-6）：单段 250ms。原双段（180ms 输入防抖 + 搜索
     effect 里再 250ms 发起延迟）合计 430ms 才发出首查，感知明显迟滞；合并后
     输入停稳 250ms 即发。命令/订阅源是本地即时过滤（直接吃原始输入 q），
     不经防抖——见 items useMemo。searching 随 debounced 同帧置真（发出即
     「搜索中」，响应落地/失败后复位），避免在搜索 effect 内同步 setState。
     同值守卫（审计返工 P2-1）：fire 时 next === debounced（窗口内删回同值）
     则搜索 effect 不会重跑，此处不得再置 searching——否则「搜索中」卡真
     永挂（finally 永不执行）。 */
  useEffect(() => {
    const t = setTimeout(() => {
      const next = q.trim();
      if (next !== debounced) {
        setDebounced(next);
        setSearching(next !== '');
      }
    }, 250);
    return () => clearTimeout(t);
  }, [q, debounced]);

  /* 文章搜索：后端 FTS5（debounced 落定即发，延迟已由上方单段防抖承担）；
     浏览器环境回退内存匹配 */
  useEffect(() => {
    const query = debounced;
    if (!query) return;
    /* F20：代际守卫——慢查询晚于新查询返回时不得覆盖新结果 */
    let alive = true;
    api
      .searchArticles(query, 10)
      .then((rows) => {
        if (!alive) return;
        setSearchError(false);
        if (!rows) {
          const lower = query.toLowerCase();
          setResults(
            useAppStore
              .getState()
              .entries.filter(
                (a) =>
                  a.title.toLowerCase().includes(lower) ||
                  a.snippet.toLowerCase().includes(lower),
              )
              .slice(0, 10),
          );
          return;
        }
        setResults(rows.map(articleRowToEntry));
      })
      .catch(() => {
        if (!alive) return;
        setSearchError(true);
        setResults([]);
      })
      .finally(() => {
        if (alive) setSearching(false);
      });
    return () => {
      alive = false;
    };
  }, [debounced, searchNonce]);

  /* 派生值：空查询时渲染归零，避免同步 setState */
  const resultsEffective = debounced ? results : EMPTY_RESULTS;
  const searchingEffective = debounced !== '' && searching;
  const searchErrorEffective = debounced ? searchError : false;
  const retrySearch = () => {
    if (!debounced) return;
    setSearchError(false);
    setSearching(true);
    setSearchNonce((nonce) => nonce + 1);
  };

  /* 命令表：全部命令（含快捷键 hint）；订阅源/文章按当前过滤 */
  const items = useMemo<PaletteItem[]>(() => {
    const s = useAppStore.getState();
    /* 命令/订阅源本地即时过滤：吃原始输入（不经防抖），打字即过滤；
       文章组仍按防抖后的 resultsEffective 呈现（上方单段 250ms）。 */
    const query = q.trim().toLowerCase();
    const out: PaletteItem[] = [];

    const commands: { label: string; hint: string; run: () => void }[] = [
      { label: refreshAllLabel(syncMode), hint: '', run: () => s.triggerManualSync() },
      { label: '将当前列表全部标为已读', hint: '', run: () => s.markCurrentViewAllRead() },
      { label: '切换 未读/全部 筛选', hint: '', run: () => s.toggleTimelineFilter() },
      { label: '切换深色 / 浅色模式', hint: '', run: () => {
        const dark = s.settings.themeMode === 'light';
        s.updateSettings({ themeMode: dark ? 'dark' : 'light' });
      } },
      { label: '添加订阅源…', hint: '', run: () => s.openAddFeedModal('') },
      { label: '新建分类…', hint: '', run: () => s.openNewCategoryModal() },
      { label: '打开设置…', hint: 'Ctrl+,', run: () => s.openSettings() },
      { label: 'AI 服务设置…', hint: '', run: () => s.openSettingsTab('ai') },
    ];
    for (const c of commands) {
      if (query && !c.label.toLowerCase().includes(query)) continue;
      out.push({ id: `cmd-${c.label}`, group: 'command', label: c.label, hint: c.hint, run: c.run });
    }

    const feedMatches = [...feedIndex.values()]
      .filter(({ feed }) =>
        !query ||
        feed.name.toLowerCase().includes(query) ||
        feed.url.toLowerCase().includes(query))
      .slice(0, 8);
    for (const { feed } of feedMatches) {
      out.push({
        id: `feed-${feed.id}`,
        group: 'feed',
        label: feed.name,
        hint: hostOf(feed.url),
        run: () => {
          useAppStore.getState().selectFeed(feed.id);
        },
      });
    }

    if (query) {
      for (const a of resultsEffective) {
        out.push({
          id: `art-${a.id}`,
          group: 'article',
          label: a.title,
          hint: (sourceIndex.get(a.feedId) ?? feedIndex.get(a.feedId))?.feed.name ?? '',
          run: () => {
            /* 文章：锚定打开——计算该文章在当前筛选下的绝对位置，从目标页
               加载列表（而非从头拉 500 篇），正确定位到很老的文章。

               【TASK-052 行为变化】顺序是「先导航、后锚定」。此前是先 anchorToArticle
               再前置导航；改造前分页/锚定的查询都不带订阅范围，两者结果相同，故顺序
               无关痛痒。per-scope 游标落地后 anchorToArticle 改为按**调用时**的
               activeFeedFilter / activeViewFilter / timelineFilter 构造查询（与
               loadMoreArticles 同一口径 scopeQueryArgs），于是旧顺序会让锚定按**旧
               范围**取位置——位置与该位置的列表不在同一筛选口径下，锚定结果错位。
               现改为按**新范围**取位置，与列表口径一致。
               归一动作由 anchorScopeNav 给出（纯函数，回归网直接断言顺序）。 */
            const st = useAppStore.getState();
            for (const step of anchorScopeNav(st)) {
              if (step.action === 'selectFeed') st.selectFeed(step.arg ?? 'all');
              else if (step.action === 'selectView') st.selectView('all');
              else st.toggleTimelineFilter();
            }
            void st.anchorToArticle(a.id);
          },
        });
      }
    }
    return out;
  }, [q, feedIndex, sourceIndex, resultsEffective, syncMode]);

  /* 光标重置：渲染期键检测模式，避免 set-state-in-effect */
  // Note: set-state-in-effect — 此处确有「effect 内 setState」；因该状态仅由本浮层的
    // 打开/关闭驱动、且需在渲染提交后同步，故按 lint 建议显式标注而非改写。
  const [prevKey, setPrevKey] = useState({ debounced, itemsLength: 0 });
  if (prevKey.debounced !== debounced || prevKey.itemsLength !== items.length) {
    setCursor(0);
    setPrevKey({ debounced, itemsLength: items.length });
  }

  useEffect(() => {
    if (listRef.current) listRef.current.scrollTop = 0;
    /* 本地过滤（命令/订阅源）随原始输入即时变化：列表内容可能在 debounced
       落定前已替换，滚动归零跟原始输入走，避免停在小防抖窗口的旧深度上。 */
  }, [q]);

  /* 键盘选中行滚动到可见（仅键盘导航触发；鼠标 hover 不滚动） */
  useEffect(() => {
    if (!keyboardNav.current) return;
    keyboardNav.current = false;
    listRef.current
      ?.querySelector<HTMLElement>(`[data-cp-index="${cursor}"]`)
      ?.scrollIntoView({ block: 'nearest' });
  }, [cursor]);

  const runItem = (it: PaletteItem | undefined) => {
    if (!it) return;
    it.run();
    onClose();
  };

  const onInputKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      if (items.length === 0) return;
      keyboardNav.current = true;
      setCursor((c) =>
        e.key === 'ArrowDown'
          ? (c + 1) % items.length
          : (c - 1 + items.length) % items.length,
      );
    } else if (e.key === 'Enter' && !e.nativeEvent.isComposing) {
      /* IME 组合中的 Enter 是选词确认，不执行 */
      e.preventDefault();
      runItem(items[cursor]);
    }
  };

  /* 分组渲染（组内序号接续全局扁平序号，键盘光标对齐） */
  let flat = -1;
  const renderGroup = (group: PaletteItem['group'], title: string) => {
    const list = items.filter((i) => i.group === group);
    if (list.length === 0) return null;
    return (
      <div key={group} role="group" aria-label={title}>
        <div className="cp-group-title" aria-hidden="true">{title}</div>
        {list.map((it) => {
          flat++;
          const idx = flat;
          return (
            <div
              key={it.id}
              data-cp-index={idx}
              className={`cp-item ${cursor === idx ? 'active' : ''}`}
              role="option"
              aria-selected={idx === cursor}
              onMouseEnter={() => setCursor(idx)}
              onClick={() => runItem(it)}
            >
              <span className="cp-ico">{group === 'command' ? <Icons.spark /> : group === 'feed' ? <Icons.rss /> : <Icons.article />}</span>
              <span className="cp-label">{it.label}</span>
              {it.hint && <span className="cp-hint">{it.hint}</span>}
            </div>
          );
        })}
      </div>
    );
  };

  return (
    <>
      <div className="search-modal-header">
        <Icons.search />
        <input
          type="text"
          aria-label="全局搜索"
          /* 由共享焦点 hook 先保存真实触发点，再聚焦；关闭态不执行。 */
          data-modal-autofocus
          placeholder="搜索文章、订阅源，或运行命令…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={onInputKeyDown}
          className="search-modal-input"
        />
        <span className="kbd-tag" style={{ marginLeft: 0 }}>Esc</span>
      </div>
      {searchErrorVisible(debounced, searchError) && (
        <div className="search-error-state" role="alert">
          <span>本地搜索失败</span>
          <button
            className="toggle-action-btn"
            aria-label="重试本地搜索"
            onClick={retrySearch}
          >
            重试
          </button>
        </div>
      )}
      <div className="cp-list" ref={listRef} role="listbox">
        {items.length === 0 ? (
          searchErrorEffective ? null : (
            <div className="search-empty">
              {searchingEffective ? '搜索中…' : '没有匹配结果'}
            </div>
          )
        ) : (
          <>
            {renderGroup('command', '操作')}
            {renderGroup('feed', '订阅源')}
            {renderGroup('article', '文章')}
          </>
        )}
      </div>
      <div className="cp-footer">
        <span><kbd>↑</kbd><kbd>↓</kbd> 选择</span>
        <span><kbd>⏎</kbd> 打开</span>
        <span><kbd>Esc</kbd> 关闭</span>
        <div style={{ flex: 1 }} />
      </div>
    </>
  );
}

/** 订阅源 URL → 域名（Papr feedHost 等价物，命令面板 hint 用） */
function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

export function Lightbox() {
  const lightboxUrl = useAppStore((s) => s.lightboxUrl);
  const lightboxEntryId = useAppStore((s) => s.lightboxEntryId);
  const lightboxPageUrl = useAppStore(s => s.lightboxPageUrl);
  const gallery = useAppStore((s) => s.lightboxGallery);
  const activeArticleId = useAppStore((s) => s.activeArticleId);
  const currentMediaKey = useAppStore((s) => {
    const item = s.entries.find((entry) => entry.id === s.lightboxGallery?.entryId);
    return item && !item.unavailable ? galleryMediaKey(item) : null;
  });
  const closeLightbox = useAppStore((s) => s.closeLightbox);
  const refreshGalleryLightbox = useAppStore((s) => s.refreshGalleryLightbox);
  const stepGalleryLightbox = useAppStore((s) => s.stepGalleryLightbox);
  const overlayRef = useRef<HTMLDivElement>(null);
  const focusedNodeRef = useRef<HTMLElement | null>(null);
  const sourceKey = gallery?.sourceKey;
  const pendingEntryId = gallery && !gallery.opened ? gallery.entryId : null;
  useEffect(() => {
    if (pendingEntryId && activeArticleId !== pendingEntryId) closeLightbox();
    else if (sourceKey && sourceKey !== currentMediaKey) refreshGalleryLightbox();
  }, [sourceKey, currentMediaKey, pendingEntryId, activeArticleId, closeLightbox, refreshGalleryLightbox]);
  /* 同 ModalOverlay：焦点移入/归还与 Tab 循环由共享 useFocusTrap 提供。 */
  const { onTrapKeyDown } = useFocusTrap(!!lightboxUrl, overlayRef);
  const current = !!gallery && sourceKey === currentMediaKey;
  const imageUrl = current && gallery.status === 'ready' ? gallery.urls[gallery.index]
    : current && gallery.status === 'loading' ? gallery.firstImageUrl : null;
  const canStep = current && gallery.status === 'ready' && gallery.urls.length > 1;
  // Note: 只接回本灯箱状态转换所失去的焦点，不抢其它模态/portal 的焦点 — 见 .agents/notes/implemented/feature/2026-11-01-模态焦点陷阱与dialog语义.md
  useLayoutEffect(() => {
    if (!lightboxUrl) { focusedNodeRef.current = null; return; }
    const panel = overlayRef.current;
    const previous = focusedNodeRef.current;
    if (!panel || !previous) return;
    const unavailable = !panel.contains(previous);
    const active = document.activeElement;
    // 分支移除内部控件时保留对话框焦点；导航按钮用 aria-disabled 保持原节点。
    if (unavailable && (active === document.body || active === previous || active === null)) {
      (panel.querySelector<HTMLElement>('button:not([disabled])') ?? panel).focus({ preventScroll: true });
    }
  }, [lightboxUrl, canStep, gallery?.status, currentMediaKey]);
  return (
    <div
      ref={overlayRef}
      tabIndex={-1}
      role="dialog"
      aria-modal="true"
      aria-label="图片预览"
      className={`modal-overlay lightbox-overlay ${lightboxUrl ? 'open' : ''}`}
      /* 同 ModalOverlay：外层常驻 DOM，关闭时只切类名，
         故必须靠 inert 让关闭态不可聚焦（见 REQ-047）。 */
      inert={!lightboxUrl}
      onClick={closeLightbox}
      onFocusCapture={(e) => { focusedNodeRef.current = e.target; }}
      onBlurCapture={(e) => {
        if (e.relatedTarget instanceof Node && e.relatedTarget !== document.body
          && !e.currentTarget.contains(e.relatedTarget)) {
          focusedNodeRef.current = null;
        }
      }}
      onKeyDown={(e) => {
        onTrapKeyDown(e);
        if (!lightboxUrl || e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229
          || e.ctrlKey || e.metaKey || e.altKey || e.shiftKey) return;
        if (e.key === 'Escape') {
          e.preventDefault(); e.stopPropagation(); closeLightbox();
        } else if (gallery && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) {
          e.preventDefault(); e.stopPropagation();
          if (canStep) stepGalleryLightbox(e.key === 'ArrowLeft' ? -1 : 1);
        }
      }}
    >
      {/* 原正文/社交单图路径仍使用共享 CoverImage，点击图片关闭语义不变。 */}
      {lightboxUrl && !gallery && (
        <CoverImage src={lightboxUrl} articleId={lightboxEntryId} pageUrl={lightboxPageUrl} className="lightbox-img" alt="preview" />
      )}
      {lightboxUrl && gallery && (
        <>
          <button className="lightbox-control lightbox-close" aria-label="关闭图片预览" title="关闭（Esc）"
            onClick={(e) => { e.stopPropagation(); closeLightbox(); }}><Icons.x /></button>
          <div className="lightbox-gallery" onClick={(e) => e.stopPropagation()}>
            {imageUrl && <GalleryPreviewImage key={`${gallery.entryId}:${gallery.mediaRevision}:${imageUrl}:${gallery.pageUrl}`}
              src={imageUrl} pageUrl={gallery.pageUrl} index={gallery.index} />}
            {(!current || gallery.status === 'loading') && (
              <div className="lightbox-status" role="status">加载图片列表…</div>
            )}
            {current && (gallery.status === 'error' || gallery.status === 'empty') && (
              <div className="lightbox-status" role="status">
                <span>{gallery.status === 'error' ? '图片列表加载失败' : '暂无图片'}</span>
                <button className="lightbox-control" onClick={(e) => {
                  e.currentTarget.closest<HTMLElement>('[role="dialog"]')?.focus({ preventScroll: true });
                  refreshGalleryLightbox();
                }}>重新加载</button>
              </div>
            )}
            <div className="lightbox-navigation">
              <button className="lightbox-control lightbox-previous" aria-label="上一张图片" aria-keyshortcuts="ArrowLeft"
                aria-disabled={!canStep} onClick={() => { if (canStep) stepGalleryLightbox(-1); }}><Icons.chevronDown /></button>
              <span className="lightbox-position" role="status" aria-live="polite" aria-atomic="true">
                {current && gallery.status === 'ready' ? `${gallery.index + 1} / ${gallery.urls.length}` : '—'}
              </span>
              <button className="lightbox-control lightbox-next" aria-label="下一张图片" aria-keyshortcuts="ArrowRight"
                aria-disabled={!canStep} onClick={() => { if (canStep) stepGalleryLightbox(1); }}><Icons.chevronDown /></button>
            </div>
          </div>
        </>
      )}
    </div>
  );
}

/** Gallery body images share transport, not the cover reporting policy. */
function GalleryPreviewImage({ src, pageUrl, index }: { src: string; pageUrl?: string; index: number }) {
  const generation = useAppStore(s => s.libraryGeneration);
  const busy = useAppStore(s => s.libraryRestoreBusy);
  const { lease, snapshot, retry } = useImageResource(src, pageUrl, !busy, 0, String(generation));
  const [loadedSrc, setLoadedSrc] = useState<string | null>(null);
  const direct = !canProxyImages() || !needsImageProxy(src);
  const resolved = snapshot.status === 'ready' ? snapshot.src : direct ? src : null;
  const failed = snapshot.status === 'failed';
  const loaded = resolved !== null && loadedSrc === resolved;
  return <div className="lightbox-gallery-image" aria-busy={!failed && !loaded}>
    {!failed && resolved && <img className="lightbox-img" src={resolved} referrerPolicy="no-referrer"
      alt={`第 ${index + 1} 张图片`} onLoad={() => setLoadedSrc(resolved)} onError={() => lease?.failDecode()} />}
    {failed ? <div className="lightbox-status" role="status">
      <span>{snapshot.reason === 'too-large' ? '图片过大，无法加载' : '图片加载失败'}</span>
      <button className="lightbox-control" onClick={event => {
        event.currentTarget.closest<HTMLElement>('[role="dialog"]')?.focus({ preventScroll: true });
        setLoadedSrc(null); retry();
      }}>重试图片</button>
    </div> : !loaded && <span className="lightbox-image-loading" role="status">图片加载中…</span>}
  </div>;
}
export function NewCategoryModal() {
  const newCategoryModalOpen = useAppStore((s) => s.newCategoryModalOpen);
  const closeMiniModal = useAppStore((s) => s.closeMiniModal);
  const createCategory = useAppStore((s) => s.createCategory);
  const [name, setName] = useState('');
  const [layout, setLayout] = useState<ContentLayoutType>('article');

  return (
    <ModalOverlay
      open={newCategoryModalOpen}
      onClose={() => closeMiniModal('newCategory')}
      ariaLabel="新建订阅分类"
    >
      <div className="mini-dialog">
        <div className="mini-dialog-title">新建订阅分类</div>

        <div className="mini-dialog-field">
          <label>分类名称</label>
          <input
            type="text"
            aria-label="分类名称"
            className="setting-input"
            style={{ width: '100%' }}
            placeholder="例如：人工智能前沿、设计影像"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </div>

        <div className="mini-dialog-field">
          <label>默认内容布局</label>
          <FluxDropdown
            width={'100%'}
            ariaLabel="默认内容布局"
            value={layout}
            onChange={(v) => setLayout(v as ContentLayoutType)}
            options={LAYOUT_OPTIONS}
          />
        </div>

        <div className="mini-dialog-actions">
          <button className="toggle-action-btn" onClick={() => closeMiniModal('newCategory')}>取消</button>
          <button
            className="toggle-action-btn btn-primary"
            /* fix-10（自检 UI-P2-3）：空名禁用主按钮（与 RenameCategoryModal 统一），
               修前「可点但静默 return」，用户不知道为什么提交不了 */
            disabled={!name.trim()}
            onClick={() => {
              if (!name.trim()) return;
              createCategory(name.trim(), layout);
              setName('');
              closeMiniModal('newCategory');
            }}
          >
            创建分类
          </button>
        </div>
      </div>
    </ModalOverlay>
  );
}

export function AddFeedModal() {
  const addFeedModalOpen = useAppStore((s) => s.addFeedModalOpen);
  const closeMiniModal = useAppStore((s) => s.closeMiniModal);
  const addFeedTargetCatId = useAppStore((s) => s.addFeedTargetCatId);
  const addFeed = useAppStore((s) => s.addFeed);
  const categories = useAppStore((s) => s.categories);
  const syncConnected = useAppStore((s) => s.syncConnected);
  const syncProtocol = useAppStore((s) => s.syncProtocol);

  /* 每次打开重挂载（key），初始值即打开瞬间的目标分类 */
  return (
    <ModalOverlay open={addFeedModalOpen} onClose={() => closeMiniModal('addFeed')} ariaLabel="添加订阅源">
      <AddFeedModalBody
        key={addFeedTargetCatId + String(addFeedModalOpen)}
        initialCatId={addFeedTargetCatId || categories[0]?.id || ''}
        categories={categories}
        syncConnected={syncConnected}
        syncProtocol={syncProtocol}
        onCancel={() => closeMiniModal('addFeed')}
        onSubmit={addFeed}
      />
    </ModalOverlay>
  );
}

function AddFeedModalBody({
  initialCatId,
  categories,
  syncConnected,
  syncProtocol,
  onCancel,
  onSubmit,
}: {
  initialCatId: string;
  categories: CategoryGroup[];
  syncConnected: boolean;
  syncProtocol: 'greader' | 'fever' | null;
  onCancel: () => void;
  onSubmit: (catId: string, url: string, title: string, layout: string, summaryMode: FeedAiMode, translateMode: FeedAiMode, syncToBackend: boolean) => void;
}) {
  const settings = useAppStore((s) => s.settings);
  const [url, setUrl] = useState('');
  const [title, setTitle] = useState('');
  const [catId, setCatId] = useState(initialCatId);
  const [layout, setLayout] = useState('inherit');
  /* OPT-011：新增源默认继承分类（默认分类仍 false，先不烧 AI）；用户可显式开/关 */
  const [summaryMode, setSummaryMode] = useState<FeedAiMode>('inherit');
  const [translateMode, setTranslateMode] = useState<FeedAiMode>('inherit');
  const [syncToBackend, setSyncToBackend] = useState(false);
  /* 连接协议切换到 Fever 时清掉本次同步选择：Fever 没有订阅写回端点，
     不能把上一次 GReader 的勾选带进提交（后端另有协议门控）。
     渲染期对账而非 effect：协议变化的下一次 commit 即清除，避免 effect
     额外一帧里客户端仍持有旧勾选。 */
  const [lastSyncProtocol, setLastSyncProtocol] = useState(syncProtocol);
  if (lastSyncProtocol !== syncProtocol) {
    setLastSyncProtocol(syncProtocol);
    if (syncProtocol === 'fever' && syncToBackend) setSyncToBackend(false);
  }
  /* 生效布局：显式选择优先，否则目标分类的布局——画廊/播客布局隐藏 AI 开关 */
  const curCat = categories.find((c) => c.id === catId);
  const curCatLayout = curCat?.layout ?? 'article';
  const effLayout = layout === 'inherit' ? curCatLayout : layout;
  const noAi = LAYOUT_NO_AI.has(effLayout);

  return (
    <div className="mini-dialog">
      <div className="mini-dialog-title">添加订阅源</div>

        <div className="mini-dialog-field">
          <label>归属分类</label>
          <FluxDropdown
            width={'100%'}
            ariaLabel="归属分类"
            value={catId}
            onChange={setCatId}
            options={categories.map((c) => ({ value: c.id, label: c.name }))}
          />
        </div>

        <div className="mini-dialog-field">
          <label>RSS/Atom 订阅地址</label>
          <input
            type="text"
            aria-label="RSS/Atom 订阅地址"
            className="setting-input"
            style={{ width: '100%' }}
            placeholder="https://example.com/rss.xml"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
          />
          <div className="mini-dialog-hint">
            {addFeedSyncHint(syncConnected, syncProtocol)}
          </div>
        </div>

        <div className="mini-dialog-field">
          <label>订阅源名称</label>
          <input
            type="text"
            aria-label="订阅源名称"
            className="setting-input"
            style={{ width: '100%' }}
            placeholder="例如：OpenAI Blog"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
          />
        </div>

        <div className="mini-dialog-field">
          <label>内容布局绑定</label>
          <FluxDropdown
            width={'100%'}
            ariaLabel="内容布局绑定"
            value={layout}
            onChange={setLayout}
            options={[
              { value: 'inherit', label: '继承分类布局' },
              ...LAYOUT_OPTIONS,
            ]}
          />
        </div>

        <div className="mini-dialog-checkbox-row">
          {!noAi && (
            <>
              <label className="mini-dialog-checkbox" style={effLayout === 'social' ? { display: 'none' } : undefined}>
                <AiModeSelect
                  mode={summaryMode}
                  categoryOn={categoryAiDefaults(settings, effLayout as ContentLayoutType, curCat).autoSummary}
                  onChange={setSummaryMode}
                  title="自动摘要：继承分类默认 / 开启 / 关闭。关闭只阻止尚未发起的自动请求，已发送的请求不会撤销。"
                  ariaLabel="自动摘要"
                />
                自动摘要
              </label>
              <label className="mini-dialog-checkbox">
                <AiModeSelect
                  mode={translateMode}
                  categoryOn={categoryAiDefaults(settings, effLayout as ContentLayoutType, curCat).autoTranslate}
                  onChange={setTranslateMode}
                  title="自动翻译：继承分类默认 / 开启 / 关闭。关闭只阻止尚未发起的自动请求，已发送的请求不会撤销。"
                  ariaLabel="自动翻译"
                />
                自动翻译
              </label>
            </>
          )}
          {addFeedShowsSyncSwitch(syncConnected, syncProtocol) && (
            <label className="mini-dialog-checkbox">
              <SwitchInline checked={syncToBackend} onChange={(v) => setSyncToBackend(v)} />
              同步到账户
            </label>
          )}
        </div>

        <div className="mini-dialog-actions">
          <button className="toggle-action-btn" onClick={onCancel}>取消</button>
          <button
            className="toggle-action-btn btn-primary"
            /* fix-10（自检 UI-P2-3）：空地址禁用主按钮（与 RenameCategoryModal 统一）。
               注意只看 url——无分类（全新安装）时不禁用：后端对 folder_id=null 有
               「自动落到未分类」兜底（见 store.addFeed fix-1）。 */
            disabled={!url.trim()}
            onClick={() => {
              if (!url.trim()) return;
              /* OPT-011：提交三态 mode（默认 inherit）；不再把 bool 回显写入。
                 noAI 布局下控件隐藏但不改写选择（布局忽略 AI；默认继承仍是产品语义）。 */
              onSubmit(catId, url.trim(), title.trim() || url.trim(), layout, summaryMode, translateMode, syncToBackend);
              onCancel();
            }}
          >
            添加订阅
          </button>
        </div>
    </div>
  );
}

/* ============================================================
   编辑源对话框：改名 / 移动分类 / 布局绑定 / AI 开关一次性提交
   ============================================================ */

export function EditFeedModal() {
  const open = useAppStore((s) => s.editFeedModalOpen);
  const targetId = useAppStore((s) => s.editFeedTargetId);
  const closeMiniModal = useAppStore((s) => s.closeMiniModal);
  const editFeed = useAppStore((s) => s.editFeed);
  const categories = useAppStore((s) => s.categories);
  const binding = useAppStore((s) => (s.editFeedTargetId ? s.feedIndex.get(s.editFeedTargetId) ?? null : null));

  return (
    <ModalOverlay open={open} onClose={() => closeMiniModal('editFeed')} ariaLabel="编辑订阅源">
      {binding && (
        <EditFeedModalBody
          key={targetId + String(open)}
          feed={binding.feed}
          curCatId={binding.cat.id}
          categories={categories}
          onCancel={() => closeMiniModal('editFeed')}
          onSubmit={(next) => {
            editFeed(targetId, next);
            closeMiniModal('editFeed');
          }}
        />
      )}
    </ModalOverlay>
  );
}

function EditFeedModalBody({
  feed,
  curCatId,
  categories,
  onCancel,
  onSubmit,
}: {
  feed: FeedItem;
  curCatId: string;
  categories: CategoryGroup[];
  onCancel: () => void;
  onSubmit: (next: { title: string; catId: string; layout: string; summaryMode: FeedAiMode; translateMode: FeedAiMode }) => void;
}) {
  const settings = useAppStore((s) => s.settings);
  const [title, setTitle] = useState(feed.name);
  const [catId, setCatId] = useState(curCatId);
  const [layout, setLayout] = useState<string>(feed.layout);
  const curCat = categories.find((c) => c.id === catId);
  const curCatLayout = curCat?.layout ?? 'article';
  /* OPT-011：显示与提交都取三态真值 */
  const [summaryMode, setSummaryMode] = useState<FeedAiMode>(normalizeFeedAiMode(feed.summaryMode));
  const [translateMode, setTranslateMode] = useState<FeedAiMode>(normalizeFeedAiMode(feed.translateMode));

  return (
    <div className="mini-dialog">
      <div className="mini-dialog-title">编辑订阅源</div>

      <div className="mini-dialog-field">
        <label>订阅源地址</label>
        <input type="text" aria-label="订阅源地址" className="setting-input" style={{ width: '100%' }} value={feed.url} disabled />
        <div className="mini-dialog-hint">地址不可修改（如需更换请删除后重新添加）</div>
      </div>

      <div className="mini-dialog-field">
        <label>订阅源名称</label>
        <input
          type="text"
          aria-label="订阅源名称"
          className="setting-input"
          style={{ width: '100%' }}
          placeholder={feed.name}
          value={title}
          onChange={(e) => setTitle(e.target.value)}
        />
      </div>

      <div className="mini-dialog-field">
        <label>归属分类</label>
        <FluxDropdown
          width={'100%'}
          ariaLabel="归属分类"
          value={catId}
          onChange={setCatId}
          options={categories.map((c) => ({ value: c.id, label: c.name }))}
        />
      </div>

      <div className="mini-dialog-field">
        <label>内容布局绑定</label>
        <FluxDropdown
          width={'100%'}
          ariaLabel="内容布局绑定"
          value={layout}
          onChange={setLayout}
          options={[
            { value: 'inherit', label: `继承分类布局（${LAYOUT_NAMES[curCatLayout as ContentLayoutType] ?? curCatLayout}）` },
            ...LAYOUT_OPTIONS,
          ]}
        />
      </div>

      <div className="mini-dialog-checkbox-row">
        {(() => {
          /* 生效布局：显式选择优先，否则继承分类布局 */
          const eff = layout === 'inherit' ? curCatLayout : layout;
          if (LAYOUT_NO_AI.has(eff)) return null;
          return (
            <>
              <label className="mini-dialog-checkbox" style={eff === 'social' ? { display: 'none' } : undefined}>
                <AiModeSelect
                  mode={summaryMode}
                  categoryOn={categoryAiDefaults(settings, eff as ContentLayoutType, curCat).autoSummary}
                  onChange={setSummaryMode}
                  title="自动摘要：继承分类默认 / 开启 / 关闭。关闭只阻止尚未发起的自动请求，已发送的请求不会撤销。"
                  ariaLabel="自动摘要"
                />
                自动摘要
              </label>
              <label className="mini-dialog-checkbox">
                <AiModeSelect
                  mode={translateMode}
                  categoryOn={categoryAiDefaults(settings, eff as ContentLayoutType, curCat).autoTranslate}
                  onChange={setTranslateMode}
                  title="自动翻译：继承分类默认 / 开启 / 关闭。关闭只阻止尚未发起的自动请求，已发送的请求不会撤销。"
                  ariaLabel="自动翻译"
                />
                自动翻译
              </label>
            </>
          );
        })()}
      </div>

      <div className="mini-dialog-actions">
        <button className="toggle-action-btn" onClick={onCancel}>取消</button>
        <button
          className="toggle-action-btn btn-primary"
          onClick={() => {
            /* OPT-011：提交三态真值。不支持 AI 的布局下控件隐藏，但**不改写**
               用户选择（沿原值提交，不把隐藏态硬翻成 off——避免只改标题时
               悄悄丢掉显式选择）。 */
            onSubmit({ title, catId, layout, summaryMode, translateMode });
          }}
        >
          保存
        </button>
      </div>
    </div>
  );
}

/* ============================================================
   分类改名对话框
   ============================================================ */

export function RenameCategoryModal() {
  const open = useAppStore((s) => s.renameCatModalOpen);
  const targetId = useAppStore((s) => s.renameCatTargetId);
  const closeMiniModal = useAppStore((s) => s.closeMiniModal);
  const renameCategory = useAppStore((s) => s.renameCategory);
  const cat = useAppStore((s) => s.categories.find((c) => c.id === s.renameCatTargetId) ?? null);

  return (
    <ModalOverlay open={open} onClose={() => closeMiniModal('renameCat')} ariaLabel="重命名分类">
      {cat && <RenameCatModalBody key={targetId + String(open)} initialName={cat.name} onCancel={() => closeMiniModal('renameCat')} onSubmit={(name) => { renameCategory(targetId, name); closeMiniModal('renameCat'); }} />}
    </ModalOverlay>
  );
}

function RenameCatModalBody({
  initialName,
  onCancel,
  onSubmit,
}: {
  initialName: string;
  onCancel: () => void;
  onSubmit: (name: string) => void;
}) {
  const [name, setName] = useState(initialName);
  return (
    <div className="mini-dialog">
      <div className="mini-dialog-title">重命名分类</div>
      <div className="mini-dialog-field">
        <label>分类名称</label>
        <input
          type="text"
          aria-label="分类名称"
          className="setting-input"
          style={{ width: '100%' }}
          value={name}
          data-modal-autofocus
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && name.trim()) onSubmit(name);
          }}
        />
      </div>
      <div className="mini-dialog-actions">
        <button className="toggle-action-btn" onClick={onCancel}>取消</button>
        <button className="toggle-action-btn btn-primary" disabled={!name.trim()} onClick={() => onSubmit(name)}>
          保存
        </button>
      </div>
    </div>
  );
}

/* ============================================================
   首次关闭询问弹窗：Rust close-ask 事件驱动。
   选项：最小化到托盘（主）/ 退出 FluxReader；「记住我的选择」默认勾选。
   记住 → 落库 closeToTray + closePromptShown（此后直接按设置走）；
   不记住 → 仅本次生效，下次关闭再问。
   ============================================================ */

export function CloseAskDialog() {
  const visible = useAppStore((s) => s.closeAskVisible);
  const answerCloseAsk = useAppStore((s) => s.answerCloseAsk);
  const dismissCloseAsk = useAppStore((s) => s.dismissCloseAsk);
  const [remember, setRemember] = useState(true);

  // Note: 遮罩/Esc 只取消询问，只有显式按钮提交关闭动作 — 见 .agents/notes/implemented/bug-fix/2026-11-04-关闭取消语义与授权关闭守卫.md
  return (
    <ModalOverlay open={visible} onClose={dismissCloseAsk} ariaLabel="关闭 FluxReader">
      <div className="mini-dialog">
        <div className="mini-dialog-title">关闭 FluxReader</div>
        <div className="mini-dialog-hint" style={{ marginTop: 0 }}>
          可以最小化到系统托盘保持后台刷新，或直接退出程序。
        </div>
        <div className="mini-dialog-checkbox-row" style={{ marginTop: 10 }}>
          <label className="mini-dialog-checkbox">
            <SwitchInline checked={remember} onChange={(v) => setRemember(v)} />
            记住我的选择（可在 设置 → 通用 修改）
          </label>
        </div>
        <div className="mini-dialog-actions">
          <button className="toggle-action-btn" onClick={() => answerCloseAsk('exit', remember)}>
            退出 FluxReader
          </button>
          <button className="toggle-action-btn btn-primary" onClick={() => answerCloseAsk('tray', remember)}>
            最小化到托盘
          </button>
        </div>
      </div>
    </ModalOverlay>
  );
}

