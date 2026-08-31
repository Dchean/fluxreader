import { useEffect, useMemo, useState } from 'react';
import { useAppStore, CONTENT_LAYOUTS, LAYOUT_NAMES } from '../store';
import { api, articleRowToEntry } from '../lib/api';
import { Icons, LayoutIcon } from './icons';
import { ModalOverlay, FluxDropdown } from './primitives';
import { formatRelativeTime } from '../lib/format';
import type { ArticleEntry, ContentLayoutType } from '../types';

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
  const selectArticle = useAppStore((s) => s.selectArticle);

  return (
    <ModalOverlay open={searchOpen} onClose={closeSearch} contentWidth={580}>
      <SearchModalBody key={String(searchOpen)} onClose={closeSearch} selectArticle={selectArticle} />
    </ModalOverlay>
  );
}

/* 每次打开重挂载（key={searchOpen}），天然获得清空的搜索词 */
function SearchModalBody({ onClose, selectArticle }: { onClose: () => void; selectArticle: (id: string) => void }) {
  const [q, setQ] = useState('');
  const [results, setResults] = useState<ArticleEntry[]>([]);
  const [searching, setSearching] = useState(false);
  const [cursor, setCursor] = useState(0);
  const feedIndex = useAppStore((s) => s.feedIndex);
  const selectFeed = useAppStore((s) => s.selectFeed);

  /* 防抖 250ms 调后端 FTS5（标题/正文/作者/AI 摘要/翻译全文）；
     Tauri 环境外回退为内存标题匹配（mock 演示用） */
  useEffect(() => {
    const query = q.trim();
    if (!query) {
      setResults([]);
      setSearching(false);
      return;
    }
    setSearching(true);
    const t = setTimeout(() => {
      api
        .searchArticles(query, 50)
        .then((rows) => {
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
                .slice(0, 50),
            );
            return;
          }
          setResults(rows.map(articleRowToEntry));
        })
        .catch(() => setResults([]))
        .finally(() => setSearching(false));
    }, 250);
    return () => clearTimeout(t);
  }, [q]);

  /* 重置键盘光标：搜索词/结果变化时回到第一项 */
  useEffect(() => setCursor(0), [q, results]);

  /* 订阅源组：名称匹配（本地 feedIndex 即可，无需后端） */
  const feedMatches = useMemo(() => {
    const query = q.trim().toLowerCase();
    if (!query) return [];
    return [...feedIndex.values()]
      .filter(({ feed }) => feed.name.toLowerCase().includes(query) || feed.url.toLowerCase().includes(query))
      .slice(0, 5);
  }, [q, feedIndex]);

  /* 命令组：固定命令表 + 关键词匹配 */
  const commands = useMemo(() => {
    const s = useAppStore.getState();
    const all: { label: string; hint: string; run: () => void }[] = [
      { label: '全部标为已读', hint: '', run: () => s.markCurrentViewAllRead() },
      { label: '切换 未读/全部 筛选', hint: '', run: () => s.toggleTimelineFilter() },
      { label: '立即同步刷新', hint: '', run: () => s.triggerManualSync() },
      { label: '打开设置', hint: 'Ctrl+,', run: () => s.openSettings() },
      { label: 'AI 服务设置', hint: '', run: () => s.openSettingsTab('ai') },
      { label: '添加订阅源', hint: '', run: () => s.openAddFeedModal('') },
      { label: '新建分类', hint: '', run: () => s.openNewCategoryModal() },
    ];
    const query = q.trim().toLowerCase();
    if (!query) return [];
    return all.filter((c) => c.label.toLowerCase().includes(query)).slice(0, 5);
  }, [q]);

  /* 扁平化全部可选项（键盘导航的目标列表）：命令 → 订阅源 → 文章（按布局分组） */
  const flatItems = useMemo(() => {
    type Item =
      | { kind: 'command'; idx: number }
      | { kind: 'feed'; idx: number }
      | { kind: 'article'; entry: ArticleEntry };
    const items: Item[] = [];
    commands.forEach((_, i) => items.push({ kind: 'command', idx: i }));
    feedMatches.forEach((_, i) => items.push({ kind: 'feed', idx: i }));
    results.forEach((entry) => items.push({ kind: 'article', entry }));
    return items;
  }, [commands, feedMatches, results]);

  /* 键盘导航：↑↓ 移动、Enter 执行、保持 input 焦点 */
  const onInputKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      if (flatItems.length === 0) return;
      setCursor((c) =>
        e.key === 'ArrowDown'
          ? (c + 1) % flatItems.length
          : (c - 1 + flatItems.length) % flatItems.length,
      );
    } else if (e.key === 'Enter') {
      e.preventDefault();
      execItem(flatItems[cursor]);
    }
  };

  const execItem = (item: { kind: 'command'; idx: number } | { kind: 'feed'; idx: number } | { kind: 'article'; entry: ArticleEntry } | undefined) => {
    if (!item) return;
    if (item.kind === 'command') {
      commands[item.idx].run();
      onClose();
    } else if (item.kind === 'feed') {
      selectFeed(feedMatches[item.idx].feed.id);
      onClose();
    } else {
      /* 文章：选中并定位列表。若该文章已读而当前是未读筛选，先切到
         「全部」视图保证卡片可见（否则定位到一条看不见的卡片）。 */
      const feedOfArticle = feedIndex.get(item.entry.feedId);
      if (feedOfArticle) {
        selectFeed(feedOfArticle.feed.id);
      }
      if (item.entry.isRead) {
        const st = useAppStore.getState();
        if (st.activeViewFilter === 'unread') st.selectView('all');
        if (st.timelineFilter === 'unread') st.toggleTimelineFilter();
      }
      selectArticle(item.entry.id);
      onClose();
    }
  };

  /* 按内容布局类型分组（搜索结果可能横跨文章/社交/播客等布局） */
  const groups = useMemo(() => {
    const byLayout = new Map<ContentLayoutType, ArticleEntry[]>();
    for (const a of results) {
      const raw = useAppStore.getState().feedIndex.get(a.feedId)?.feed.layout ?? 'article';
      const layout: ContentLayoutType = raw === 'inherit' ? 'article' : raw;
      const list = byLayout.get(layout) ?? [];
      list.push(a);
      byLayout.set(layout, list);
    }
    return [...byLayout.entries()];
  }, [results]);

  /* 文章项的全局序号（键盘光标对齐）：命令数 + 订阅源数 + 组内偏移 */
  let articleSeq = commands.length + feedMatches.length;

  return (
    <>
      <div className="search-modal-header">
        <Icons.search />
        <input
          type="text"
          autoFocus
          placeholder="搜索文章、订阅源，或输入命令…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={onInputKeyDown}
          className="search-modal-input"
        />
        <span className="kbd-tag" style={{ marginLeft: 0 }}>ESC 关闭</span>
      </div>
      <div className="search-modal-results">
        {/* 命令组 */}
        {commands.length > 0 && (
          <div>
            <div className="search-group-label">命令 · {commands.length}</div>
            {commands.map((c, i) => (
              <div
                key={c.label}
                className={`search-result-item ${cursor === i ? 'active' : ''}`}
                onMouseEnter={() => setCursor(i)}
                onClick={() => execItem({ kind: 'command', idx: i })}
              >
                <div className="search-result-title">{c.label}</div>
                {c.hint && <div className="search-result-meta">{c.hint}</div>}
              </div>
            ))}
          </div>
        )}
        {/* 订阅源组 */}
        {feedMatches.length > 0 && (
          <div>
            <div className="search-group-label">订阅源 · {feedMatches.length}</div>
            {feedMatches.map(({ feed }, i) => {
              const seq = commands.length + i;
              return (
                <div
                  key={feed.id}
                  className={`search-result-item ${cursor === seq ? 'active' : ''}`}
                  onMouseEnter={() => setCursor(seq)}
                  onClick={() => execItem({ kind: 'feed', idx: i })}
                >
                  <div className="search-result-title">{feed.name}</div>
                  <div className="search-result-meta">{feed.url}</div>
                </div>
              );
            })}
          </div>
        )}
        {/* 文章组（按布局分组） */}
        {groups.map(([layout, items]) => (
          <div key={layout}>
            <div className="search-group-label">
              {LAYOUT_NAMES[layout]} · {items.length}
            </div>
            {items.map((a) => {
              const seq = articleSeq++;
              return (
                <div
                  key={a.id}
                  className={`search-result-item ${cursor === seq ? 'active' : ''}`}
                  onMouseEnter={() => setCursor(seq)}
                  onClick={() => execItem({ kind: 'article', entry: a })}
                >
                  <div className="search-result-title">{a.title}</div>
                  <div className="search-result-meta">
                    {feedIndex.get(a.feedId)?.feed.name ?? ''} · {formatRelativeTime(a.publishedAt)}
                  </div>
                </div>
              );
            })}
          </div>
        ))}
        {!searching && q.trim() && results.length === 0 && commands.length === 0 && feedMatches.length === 0 && (
          <div className="search-empty">未找到匹配内容</div>
        )}
        {searching && <div className="search-empty">搜索中…</div>}
      </div>
    </>
  );
}

export function Lightbox() {
  const lightboxUrl = useAppStore((s) => s.lightboxUrl);
  const closeLightbox = useAppStore((s) => s.closeLightbox);
  return (
    <div className={`modal-overlay lightbox-overlay ${lightboxUrl ? 'open' : ''}`} onClick={closeLightbox}>
      {lightboxUrl && (
        <img
          src={lightboxUrl}
          className="lightbox-img"
          alt="preview"
        />
      )}
    </div>
  );
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
    >
      <div className="mini-dialog">
        <div className="mini-dialog-title">新建订阅分类</div>

        <div className="mini-dialog-field">
          <label>分类名称</label>
          <input
            type="text"
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
            value={layout}
            onChange={(v) => setLayout(v as ContentLayoutType)}
            options={LAYOUT_OPTIONS}
          />
        </div>

        <div className="mini-dialog-actions">
          <button className="toggle-action-btn" onClick={() => closeMiniModal('newCategory')}>取消</button>
          <button
            className="toggle-action-btn btn-primary"
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

  /* 每次打开重挂载（key），初始值即打开瞬间的目标分类 */
  return (
    <ModalOverlay open={addFeedModalOpen} onClose={() => closeMiniModal('addFeed')}>
      <AddFeedModalBody
        key={addFeedTargetCatId + String(addFeedModalOpen)}
        initialCatId={addFeedTargetCatId || categories[0]?.id || ''}
        categories={categories}
        onCancel={() => closeMiniModal('addFeed')}
        onSubmit={addFeed}
      />
    </ModalOverlay>
  );
}

function AddFeedModalBody({
  initialCatId,
  categories,
  onCancel,
  onSubmit,
}: {
  initialCatId: string;
  categories: { id: string; name: string }[];
  onCancel: () => void;
  onSubmit: (catId: string, url: string, title: string, layout: string, autoSummary: boolean, autoTranslate: boolean) => void;
}) {
  const [url, setUrl] = useState('');
  const [title, setTitle] = useState('');
  const [catId, setCatId] = useState(initialCatId);
  const [layout, setLayout] = useState('inherit');
  const [autoSummary, setAutoSummary] = useState(true);
  const [autoTranslate, setAutoTranslate] = useState(false);

  return (
    <div className="mini-dialog">
      <div className="mini-dialog-title">添加订阅源</div>

        <div className="mini-dialog-field">
          <label>归属分类</label>
          <FluxDropdown
            width={'100%'}
            value={catId}
            onChange={setCatId}
            options={categories.map((c) => ({ value: c.id, label: c.name }))}
          />
        </div>

        <div className="mini-dialog-field">
          <label>RSS/Atom 订阅地址</label>
          <input
            type="text"
            className="setting-input"
            style={{ width: '100%' }}
            placeholder="https://example.com/rss.xml"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
          />
          <div className="mini-dialog-hint">
            不连接 Miniflux 也可添加：客户端将直连源站抓取（第一优先级），
            连接后自动同步订阅关系并兜底直连失败的源。
          </div>
        </div>

        <div className="mini-dialog-field">
          <label>订阅源名称</label>
          <input
            type="text"
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
            value={layout}
            onChange={setLayout}
            options={[
              { value: 'inherit', label: '继承分类布局' },
              ...LAYOUT_OPTIONS,
            ]}
          />
        </div>

        <div className="mini-dialog-checkbox-row">
          <label className="mini-dialog-checkbox">
            <input
              type="checkbox"
              checked={autoSummary}
              onChange={(e) => setAutoSummary(e.target.checked)}
              style={{ accentColor: 'var(--accent)' }}
            />
            自动摘要
          </label>
          <label className="mini-dialog-checkbox">
            <input
              type="checkbox"
              checked={autoTranslate}
              onChange={(e) => setAutoTranslate(e.target.checked)}
              style={{ accentColor: 'var(--accent)' }}
            />
            自动翻译
          </label>
        </div>

        <div className="mini-dialog-actions">
          <button className="toggle-action-btn" onClick={onCancel}>取消</button>
          <button
            className="toggle-action-btn btn-primary"
            onClick={() => {
              if (!url.trim()) return;
              onSubmit(catId, url.trim(), title.trim() || url.trim(), layout, autoSummary, autoTranslate);
              onCancel();
            }}
          >
            添加订阅
          </button>
        </div>
    </div>
  );
}
