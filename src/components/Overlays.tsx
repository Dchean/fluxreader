import { useEffect, useMemo, useRef, useState } from 'react';
import { useAppStore, CONTENT_LAYOUTS, LAYOUT_NAMES } from '../store';
import { api, articleRowToEntry } from '../lib/api';
import { Icons, LayoutIcon } from './icons';
import { ModalOverlay, FluxDropdown } from './primitives';
import type { ArticleEntry, ContentLayoutType, FeedItem } from '../types';

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
    <ModalOverlay open={searchOpen} onClose={closeSearch} contentWidth={580}>
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
  const [cursor, setCursor] = useState(0);
  const [searchError, setSearchError] = useState(false);
  const feedIndex = useAppStore((s) => s.feedIndex);
  const listRef = useRef<HTMLDivElement>(null);
  /* 键盘移动标记：scrollIntoView 只在键盘导航时触发 */
  const keyboardNav = useRef(false);

  /* 180ms 防抖 */
  useEffect(() => {
    const t = setTimeout(() => setDebounced(q.trim()), 180);
    return () => clearTimeout(t);
  }, [q]);

  /* 文章搜索：后端 FTS5（防抖后触发）；浏览器环境回退内存匹配 */
  useEffect(() => {
    const query = debounced;
    if (!query) {
      setResults([]);
      setSearching(false);
      setSearchError(false);
      return;
    }
    setSearching(true);
    const t = setTimeout(() => {
      api
        .searchArticles(query, 10)
        .then((rows) => {
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
          setSearchError(true);
          setResults([]);
        })
        .finally(() => setSearching(false));
    }, 250);
    return () => clearTimeout(t);
  }, [debounced]);

  /* 命令表：全部命令（含快捷键 hint）；订阅源/文章按当前过滤 */
  const items = useMemo<PaletteItem[]>(() => {
    const s = useAppStore.getState();
    const query = debounced.toLowerCase();
    const out: PaletteItem[] = [];

    const commands: { label: string; hint: string; run: () => void }[] = [
      { label: '同步并刷新所有订阅源', hint: '', run: () => s.triggerManualSync() },
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
      for (const a of results) {
        out.push({
          id: `art-${a.id}`,
          group: 'article',
          label: a.title,
          hint: feedIndex.get(a.feedId)?.feed.name ?? '',
          run: () => {
            /* 文章：选中并定位列表。若该文章已读而当前是未读筛选，先切到
               「全部」视图保证卡片可见（否则定位到一条看不见的卡片）。 */
            const st = useAppStore.getState();
            const binding = st.feedIndex.get(a.feedId);
            if (binding) st.selectFeed(binding.feed.id);
            if (a.isRead) {
              if (st.activeViewFilter === 'unread') st.selectView('all');
              if (st.timelineFilter === 'unread') st.toggleTimelineFilter();
            }
            st.selectArticle(a.id);
          },
        });
      }
    }
    return out;
  }, [debounced, feedIndex, results]);

  /* 光标重置 + 查询变化回到列表顶部 */
  useEffect(() => setCursor(0), [debounced, items.length]);
  useEffect(() => {
    if (listRef.current) listRef.current.scrollTop = 0;
  }, [debounced]);

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
          autoFocus
          placeholder="搜索文章、订阅源，或运行命令…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={onInputKeyDown}
          className="search-modal-input"
        />
        <span className="kbd-tag" style={{ marginLeft: 0 }}>ESC 关闭</span>
      </div>
      <div className="cp-list" ref={listRef} role="listbox">
        {items.length === 0 ? (
          <div className="search-empty">
            {searching ? '搜索中…' : searchError ? '搜索失败 — 请检查网络连接' : '没有结果'}
          </div>
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
        <span><kbd>esc</kbd> 关闭</span>
        <div style={{ flex: 1 }} />
        <span>支持文章 · 订阅源 · 命令</span>
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
  const closeLightbox = useAppStore((s) => s.closeLightbox);
  return (
    <div className={`modal-overlay lightbox-overlay ${lightboxUrl ? 'open' : ''}`} onClick={closeLightbox}>
      {lightboxUrl && (
        <img
          src={lightboxUrl}
          className="lightbox-img"
          alt="preview"
          referrerPolicy="no-referrer"
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
    <ModalOverlay open={open} onClose={() => closeMiniModal('editFeed')}>
      {binding && (
        <EditFeedModalBody
          key={targetId + String(open)}
          feed={binding.feed}
          curCatId={binding.cat.id}
          curCatLayout={binding.cat.layout}
          categories={categories.map((c) => ({ id: c.id, name: c.name, layout: c.layout }))}
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
  curCatLayout,
  categories,
  onCancel,
  onSubmit,
}: {
  feed: FeedItem;
  curCatId: string;
  curCatLayout: string;
  categories: { id: string; name: string; layout: string }[];
  onCancel: () => void;
  onSubmit: (next: { title: string; catId: string; layout: string; autoSummary: boolean; autoTranslate: boolean }) => void;
}) {
  const [title, setTitle] = useState(feed.name);
  const [catId, setCatId] = useState(curCatId);
  const [layout, setLayout] = useState<string>(feed.layout);
  const [autoSummary, setAutoSummary] = useState(feed.autoSummary);
  const [autoTranslate, setAutoTranslate] = useState(feed.autoTranslate);

  return (
    <div className="mini-dialog">
      <div className="mini-dialog-title">编辑订阅源</div>

      <div className="mini-dialog-field">
        <label>订阅源地址</label>
        <input type="text" className="setting-input" style={{ width: '100%' }} value={feed.url} disabled />
        <div className="mini-dialog-hint">地址不可修改（如需更换请删除后重新添加）</div>
      </div>

      <div className="mini-dialog-field">
        <label>订阅源名称</label>
        <input
          type="text"
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
          value={catId}
          onChange={setCatId}
          options={categories.map((c) => ({ value: c.id, label: c.name }))}
        />
      </div>

      <div className="mini-dialog-field">
        <label>内容布局绑定</label>
        <FluxDropdown
          width={'100%'}
          value={layout}
          onChange={setLayout}
          options={[
            { value: 'inherit', label: `继承分类布局（${LAYOUT_NAMES[curCatLayout as ContentLayoutType] ?? curCatLayout}）` },
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
          onClick={() => onSubmit({ title, catId, layout, autoSummary, autoTranslate })}
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
    <ModalOverlay open={open} onClose={() => closeMiniModal('renameCat')}>
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
          className="setting-input"
          style={{ width: '100%' }}
          value={name}
          autoFocus
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
  const [remember, setRemember] = useState(true);

  return (
    <ModalOverlay open={visible} onClose={() => answerCloseAsk('tray', remember)}>
      <div className="mini-dialog">
        <div className="mini-dialog-title">关闭 FluxReader</div>
        <div className="mini-dialog-hint" style={{ marginTop: 0 }}>
          可以最小化到系统托盘保持后台刷新，或直接退出程序。
        </div>
        <div className="mini-dialog-checkbox-row" style={{ marginTop: 10 }}>
          <label className="mini-dialog-checkbox">
            <input
              type="checkbox"
              checked={remember}
              onChange={(e) => setRemember(e.target.checked)}
              style={{ accentColor: 'var(--accent)' }}
            />
            记住我的选择（之后可在 设置 → 通用 修改）
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
