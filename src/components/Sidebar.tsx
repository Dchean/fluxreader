import { useShallow } from 'zustand/react/shallow';
import {
  useAppStore,
  CONTENT_LAYOUTS,
  LAYOUT_NAMES,
  VIEW_NAMES,
  selectViewCounts,
  selectTreeCounts,
  resolveFeedLayout,
} from '../store';
import { Icons } from './icons';
import type { ContentLayoutType, ViewFilterType } from '../types';

/* ============================================================
   Sidebar —— 视图过滤 / 内容布局 / 订阅树 / 同步状态 / 设置入口
   对应原型 §4.1 与规范 §4.1

   计数口径（统一）：
   - 视图角标 = 当前布局 × 订阅范围 下的 all/today/unread/starred 数
   - 树角标（全部行/分类行/订阅源行）= 该行在「当前布局 × 当前视图」
     筛选下的条目数 —— 与列表内容同源，点进去看到的就是这个数
   ============================================================ */

const VIEW_ITEMS: { id: ViewFilterType; label: string; icon: () => React.ReactElement }[] = [
  { id: 'all', label: VIEW_NAMES.all, icon: Icons.layers },
  { id: 'today', label: VIEW_NAMES.today, icon: Icons.clock },
  { id: 'unread', label: VIEW_NAMES.unread, icon: Icons.unreadDot },
  { id: 'starred', label: VIEW_NAMES.starred, icon: Icons.star },
];

const LAYOUT_ITEMS: { id: ContentLayoutType; label: string; icon: () => React.ReactElement }[] = CONTENT_LAYOUTS.map(
  (id) => ({ id, label: LAYOUT_NAMES[id], icon: Icons[id] }),
);

export function Sidebar() {
  const categories = useAppStore((s) => s.categories);
  const activeContentLayout = useAppStore((s) => s.activeContentLayout);
  const activeViewFilter = useAppStore((s) => s.activeViewFilter);
  const activeFeedFilter = useAppStore((s) => s.activeFeedFilter);
  const syncStatus = useAppStore((s) => s.syncStatus);
  const minifluxConnected = useAppStore((s) => s.minifluxConnected);

  const selectLayout = useAppStore((s) => s.selectLayout);
  const selectView = useAppStore((s) => s.selectView);
  const selectFeed = useAppStore((s) => s.selectFeed);
  const openSearch = useAppStore((s) => s.openSearch);
  const openSettings = useAppStore((s) => s.openSettings);
  const openSettingsTab = useAppStore((s) => s.openSettingsTab);
  const openNewCategoryModal = useAppStore((s) => s.openNewCategoryModal);
  const openAddFeedModal = useAppStore((s) => s.openAddFeedModal);
  const toggleFolderCollapse = useAppStore((s) => s.toggleFolderCollapse);
  const toggleAllFolders = useAppStore((s) => s.toggleAllFolders);
  const triggerManualSync = useAppStore((s) => s.triggerManualSync);

  /* 返回新引用的派生 selector 必须包 useShallow，否则 useSyncExternalStore 无限重渲染 */
  const badge = useAppStore(useShallow(selectViewCounts));
  const treeCounts = useAppStore(useShallow(selectTreeCounts));

  /* 仅展示与当前布局匹配的分类组及其子源（分类布局匹配 or 任一子源覆盖匹配） */
  const filteredCategories = categories.filter(
    (c) => c.layout === activeContentLayout || c.feeds.some((f) => resolveFeedLayout(f, c.layout) === activeContentLayout),
  );

  const syncLabel =
    syncStatus === 'syncing'
      ? '正在同步...'
      : syncStatus === 'error'
        ? '同步失败'
        : minifluxConnected
          ? 'Miniflux 已同步'
          : '本地模式 · 直连抓取';

  return (
    <aside className="sidebar">
      <div className="sidebar-fixed-top">
        <div className="sidebar-brand">
          <div className="brand-icon">F</div>
          <div>
            <div className="brand-name">FluxReader</div>
            <div className="brand-badge">Local-First</div>
          </div>
        </div>

        <div className="sidebar-search-pill" onClick={openSearch} role="button" tabIndex={0}
          onKeyDown={(e) => e.key === 'Enter' && openSearch()}>
          <Icons.search />
          <span>全局搜索...</span>
          <span className="kbd-tag">Ctrl K</span>
        </div>

        {/* Section 1: 视图 */}
        <div className="nav-section-title">视图</div>
        {VIEW_ITEMS.map((v) => (
          <button
            key={v.id}
            className={`nav-tab-item ${activeViewFilter === v.id ? 'active-view' : ''}`}
            onClick={() => selectView(v.id)}
          >
            <span className="nav-icon"><v.icon /></span>
            <span>{v.label}</span>
            <span className="count-badge">{badge[v.id]}</span>
          </button>
        ))}

        {/* Section 2: 内容布局 */}
        <div className="nav-section-title" style={{ marginTop: 10 }}>内容布局</div>
        {LAYOUT_ITEMS.map((l) => (
          <button
            key={l.id}
            className={`nav-tab-item ${activeContentLayout === l.id ? 'active-layout' : ''}`}
            onClick={() => selectLayout(l.id)}
          >
            <span className="nav-icon"><l.icon /></span>
            <span>{l.label}</span>
          </button>
        ))}

        {/* Section 3: 订阅源工具栏 */}
        <div className="nav-section-title" style={{ marginTop: 10 }}>
          <span>订阅源</span>
          <div className="feed-header-actions">
            <button className="icon-sub-btn" onClick={toggleAllFolders} title="展开/收起全部">
              <Icons.eye />
            </button>
            <button className="icon-sub-btn" onClick={openNewCategoryModal} title="新建分类">
              <Icons.newFolder />
            </button>
            <button
              className="icon-sub-btn"
              onClick={() => openAddFeedModal(categories[0]?.id ?? '')}
              title="添加订阅源"
            >
              <Icons.plus />
            </button>
          </div>
        </div>
      </div>

      {/* 中部滚动订阅树 */}
      <div className="sidebar-scrollable-feeds">
        <button
          className={`feed-leaf-item feed-all-row ${activeFeedFilter === 'all' ? 'active-feed' : ''}`}
          onClick={() => selectFeed('all')}
        >
          <div className="feed-leaf-name">
            <span className="feed-favicon-fallback"><Icons.grid /></span>
            <span className="feed-leaf-title" style={{ fontWeight: 600 }}>全部订阅源</span>
          </div>
          <span className="feed-count-badge">{treeCounts.get('all') ?? 0}</span>
        </button>

        {filteredCategories.length === 0 && (
          <div style={{ fontSize: 11, color: 'var(--text-tertiary)', padding: '10px 8px' }}>
            该布局下暂无分类
          </div>
        )}

        {filteredCategories.map((cat) => {
          const matchingFeeds = cat.feeds.filter(
            (f) => resolveFeedLayout(f, cat.layout) === activeContentLayout,
          );
          return (
            <div className="feed-group-tree" key={cat.id}>
              <div className={`feed-folder-header ${activeFeedFilter === cat.id ? 'active-feed' : ''}`}>
                <div
                  className="feed-folder-title"
                  onClick={() => selectFeed(cat.id)}
                  role="button"
                  tabIndex={0}
                  onKeyDown={(e) => e.key === 'Enter' && selectFeed(cat.id)}
                >
                  <span className="nav-icon"><Icons.folder /></span>
                  <span>{cat.name}</span>
                </div>
                <div className="feed-folder-right">
                  <span className="feed-count-badge" title="当前视图筛选下的条目数">{treeCounts.get(cat.id) ?? 0}</span>
                  <button
                    className={`folder-chevron-btn ${cat.collapsed ? 'collapsed' : ''}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      toggleFolderCollapse(cat.id);
                    }}
                    title={cat.collapsed ? '展开' : '收起'}
                    aria-expanded={!cat.collapsed}
                  >
                    <Icons.chevronDown />
                  </button>
                </div>
              </div>
              {!cat.collapsed && (
                <div className="feed-sub-list">
                  {matchingFeeds.map((f) => (
                    <button
                      key={f.id}
                      className={`feed-leaf-item ${activeFeedFilter === f.id ? 'active-feed' : ''}`}
                      onClick={() => selectFeed(f.id)}
                      title={f.name}
                    >
                      <div className="feed-leaf-name">
                        {f.favicon ? (
                          <img
                            src={f.favicon}
                            className="feed-favicon"
                            alt=""
                            loading="lazy"
                            onError={(e) => {
                              (e.target as HTMLImageElement).style.display = 'none';
                            }}
                          />
                        ) : (
                          <span className="feed-favicon-fallback"><Icons.dot /></span>
                        )}
                        <span className="feed-leaf-title">{f.name}</span>
                      </div>
                      <span className="feed-count-badge" title="当前视图筛选下的条目数">{treeCounts.get(f.id) ?? 0}</span>
                    </button>
                  ))}
                </div>
              )}
            </div>
          );
        })}
      </div>

      {/* 底部固定区 */}
      <div className="sidebar-fixed-bottom">
        <div className="sync-status-row">
          <button className="sync-status-pill" onClick={() => openSettingsTab('sync')}>
            <div className={`sync-dot ${minifluxConnected ? '' : 'sync-dot-off'}`} />
            <span>{syncLabel}</span>
          </button>
          <button
            className={`sync-refresh-btn ${syncStatus === 'syncing' ? 'spinning' : ''}`}
            onClick={triggerManualSync}
            title="手动同步"
          >
            <Icons.refresh />
          </button>
        </div>
        <button className="nav-tab-item" onClick={openSettings}>
          <span className="nav-icon"><Icons.settings /></span>
          <span>设置中心</span>
          <span className="kbd-tag">Ctrl ,</span>
        </button>
      </div>
    </aside>
  );
}
