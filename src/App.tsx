import { useEffect, useState } from 'react';
import { useAppStore, bootstrapGithubAuth, selectVisibleEntries } from './store';
import { Sidebar } from './components/Sidebar';
import { Timeline } from './components/Timeline';
import { Reader } from './components/Reader';
import { PlayerBar } from './components/PlayerBar';
import { SettingsModal } from './components/SettingsModal';
import { SearchModal, Lightbox, NewCategoryModal, AddFeedModal, EditFeedModal, RenameCategoryModal, CloseAskDialog } from './components/Overlays';
import { ContextMenuHost } from './components/ContextMenu';
/* ============================================================
   Application Shell

   窗口控制：在 Tauri 窗口内调用原生窗口 API；
   浏览器开发模式下降级为无操作，避免报错。
   ============================================================ */

async function getCurrentWindowApi() {
  /* 动态 import：浏览器环境（无 Tauri IPC）不会打包失败 */
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    return getCurrentWindow();
  } catch {
    return null;
  }
}

const minimizeWindow = async () => {
  const win = await getCurrentWindowApi();
  if (win) await win.minimize();
};

const toggleMaximizeWindow = async () => {
  const win = await getCurrentWindowApi();
  if (win) await win.toggleMaximize();
};

const closeWindow = async () => {
  const win = await getCurrentWindowApi();
  if (win) await win.close();
};

export default function App() {
  const activeContentLayout = useAppStore((s) => s.activeContentLayout);
  const themeMode = useAppStore((s) => s.settings.themeMode);
  const palette = useAppStore((s) => s.settings.palette);
  const listWidth = useAppStore((s) => s.settings.listWidth);
  /* TASK-067 N9：拖动期间的本地列宽（松手前不落库） */
  const [dragWidth, setDragWidth] = useState<number | null>(null);
  const effectiveListWidth = dragWidth ?? listWidth;
  const updateSettings = useAppStore((s) => s.updateSettings);
  const toasts = useAppStore((s) => s.toasts);
  const playerActive = useAppStore((s) => s.player.isActive);
  const playerExpanded = useAppStore((s) => s.playerExpanded);
  const dataLoading = useAppStore((s) => s.dataLoading);
  const bootstrapError = useAppStore((s) => s.bootstrapError);

  /* PlayerBar 活跃 → body 标记类（toast 层上移避让底栏） */
  useEffect(() => {
    document.body.classList.toggle('has-player', playerActive);
  }, [playerActive]);

  /* 全屏播放器展开（REQ-004）：迷你播放条隐藏，toast 层须回到右下贴底——
     否则 has-player 的 96px 抬高会让 toast 悬浮遮挡播放器中下部 */
  useEffect(() => {
    document.body.classList.toggle('has-player-expanded', playerExpanded && playerActive);
  }, [playerExpanded, playerActive]);

  /* ---------- 数据源装载：Tauri 环境从 SQLite 拉全量；浏览器保持 mock ---------- */
  useEffect(() => {
    /* 设置恢复先于数据装载：主题/视图等在首帧就位，避免闪默认值 */
    void useAppStore.getState().bootstrapSettings().then(() => {
      useAppStore.getState().bootstrapFromBackend();
    });
    /* GitHub 登录态恢复（设备流 token 持久化在 SQLite，启动即还原账户显示） */
    void bootstrapGithubAuth();
  }, []);

  /* ---------- 后台刷新调度器事件：新文章到达即重载列表 + toast ---------- */
  useEffect(() => {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const { listen } = await import('@tauri-apps/api/event');
        unlisten = await listen<{ new_articles: number; failed_feeds: number }>(
          'feeds-updated',
          (e) => {
            const { new_articles, failed_feeds } = e.payload;
            void useAppStore.getState().reloadFromBackend();
            if (new_articles > 0) {
              useAppStore.getState().showToast(`后台刷新：新文章 ${new_articles} 篇`);
            } else if (failed_feeds > 0) {
              useAppStore.getState().showToast(`后台刷新：${failed_feeds} 个源抓取失败`);
            }
          },
        );
      } catch {
        /* 事件监听失败不影响主流程 */
      }
    })();
    return () => unlisten?.();
  }, []);

  /* ---------- 后台 Miniflux 自动同步进行中状态（侧栏 spinner 可见性） ---------- */
  useEffect(() => {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
    const unlistens: Array<() => void> = [];
    void (async () => {
      try {
        const { listen } = await import('@tauri-apps/api/event');
        unlistens.push(
          await listen('sync-running', () => {
            useAppStore.setState({ backgroundSyncing: true });
          }),
        );
        unlistens.push(
          await listen('sync-idle', () => {
            useAppStore.setState({ backgroundSyncing: false });
          }),
        );
      } catch {
        /* 事件监听失败不影响主流程 */
      }
    })();
    return () => unlistens.forEach((u) => u());
  }, []);

  /* ---------- SMTC 系统媒体键回调：媒体键/音量浮层控制播放 ---------- */
  useEffect(() => {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const { listen } = await import('@tauri-apps/api/event');
        unlisten = await listen<string>('player-media', (e) => {
          const st = useAppStore.getState();
          if (!st.player.isActive) return;
          if (e.payload === 'toggle' || e.payload === 'play' || e.payload === 'pause') {
            st.togglePlayerPlay();
          } else if (e.payload === 'stop') {
            st.closePodcastBar();
          }
        });
      } catch {
        /* 事件监听失败不影响主流程 */
      }
    })();
    return () => unlisten?.();
  }, []);

  /* ---------- 首次关闭询问：Rust CloseRequested 判断未问过 → 弹窗 ---------- */
  useEffect(() => {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const { listen, emit } = await import('@tauri-apps/api/event');
        unlisten = await listen('close-ask', () => {
          useAppStore.setState({ closeAskVisible: true });
          /* 回 ack：让 Rust 侧知道前端已接管（10s 无 ack Rust 会兜底隐藏，
             避免窗口"关不掉"；ack 前提是弹窗真的弹出来了） */
          void emit('close-ask-ack', 'frontend-ready');
        });
      } catch {
        /* 忽略：监听失败时 Rust 10s 兜底按默认行为处理 */
      }
    })();
    return () => unlisten?.();
  }, []);

  /* ---------- 主题引擎：data-theme × data-palette 应用到 <html> ---------- */
  useEffect(() => {
    const root = document.documentElement;
    root.setAttribute('data-palette', palette);
    if (themeMode === 'auto') {
      const mq = window.matchMedia('(prefers-color-scheme: dark)');
      root.setAttribute('data-theme', mq.matches ? 'dark' : 'light');
      const onChange = (e: MediaQueryListEvent) => root.setAttribute('data-theme', e.matches ? 'dark' : 'light');
      mq.addEventListener('change', onChange);
      return () => mq.removeEventListener('change', onChange);
    }
    root.setAttribute('data-theme', themeMode);
  }, [themeMode, palette]);

  /* ---------- 全局快捷键 ---------- */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = useAppStore.getState();
      const target = e.target as HTMLElement;
      const inInput = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable;

      if (e.ctrlKey && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        if (s.searchOpen) s.closeSearch(); else s.openSearch();
        return;
      }
      if (e.ctrlKey && e.key === ',') {
        e.preventDefault();
        if (s.settingsOpen) s.closeSettings(); else s.openSettings();
        return;
      }
      if (e.key === 'Escape') {
        /* 浮层从最顶层开始依次关闭 */
        if (s.searchOpen) s.closeSearch();
        else if (s.newCategoryModalOpen) s.closeMiniModal('newCategory');
        else if (s.addFeedModalOpen) s.closeMiniModal('addFeed');
        else if (s.editFeedModalOpen) s.closeMiniModal('editFeed');
        else if (s.renameCatModalOpen) s.closeMiniModal('renameCat');
        else if (s.settingsOpen) s.closeSettings();
        else if (s.lightboxUrl) s.closeLightbox();
        else if (s.playerExpanded && s.player.isActive) s.togglePlayerExpanded(); // Full Player 大浮层
        else if (s.activeArticleId) s.clearReaderSelection();
        return;
      }
      if (inInput) return;

      /* P3[F2]（REQ-104）：任一浮层打开时，**单键快捷键**（S/M/J/K）一律让路。
         此前只有 Space 判断了 defaultPrevented，S/M/J/K 不看浮层状态——设置页、
         搜索框、各类弹窗打开时，焦点若不在输入框上，按 S/M 会作用到**浮层背后的
         当前文章**（改了它的收藏/已读却看不见），J/K 还会在背后切换文章。
         Ctrl+K / Ctrl+, / Escape 属浮层自身操作，在此判断之前已处理，不受影响。 */
      const overlayOpen =
        s.searchOpen || s.settingsOpen || s.newCategoryModalOpen || s.addFeedModalOpen ||
        s.editFeedModalOpen || s.renameCatModalOpen || !!s.lightboxUrl ||
        (s.playerExpanded && s.player.isActive);
      if (overlayOpen && !e.ctrlKey && !e.metaKey && !e.altKey) {
        const singleKey = ['s', 'S', 'm', 'M', 'j', 'k'].includes(e.key);
        if (singleKey) return;
      }

      /* Space：播放器激活时播放/暂停（快捷键表承诺）。
         但若该按键已被更具体的控件消费（卡片/下拉/菜单等补了 role+tabIndex 后
         可按 Space 激活自身，见 REQ-008），则让路——否则同一次 Space 会
         既执行控件动作、又切换播放（双重动作）。
         只在此分支判断 defaultPrevented，不动其它快捷键的既有语义。 */
      if (e.key === ' ' && s.player.isActive) {
        if (e.defaultPrevented) return;
        e.preventDefault();
        s.togglePlayerPlay();
        return;
      }

      if ((e.key === 's' || e.key === 'S') && s.activeArticleId) {
        s.toggleCurrentStar();
        return;
      }
      if ((e.key === 'm' || e.key === 'M') && s.activeArticleId) {
        s.toggleCurrentReadStatus();
        return;
      }
      /* J/K 键盘流：选中即打开（已确认的产品决策） */
      if (e.key === 'j' || e.key === 'k') {
        if (s.activeContentLayout !== 'article') return;
        const items = selectVisibleEntries(s);
        const curIdx = items.findIndex((a) => a.id === s.activeArticleId);
        let nextIdx: number;
        if (curIdx === -1) {
          nextIdx = e.key === 'j' ? 0 : items.length - 1;
        } else {
          nextIdx = e.key === 'j'
            ? (curIdx < items.length - 1 ? curIdx + 1 : 0)
            : (curIdx > 0 ? curIdx - 1 : items.length - 1);
        }
        if (items[nextIdx]) s.selectArticle(items[nextIdx].id);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const gridClass = activeContentLayout === 'article' ? 'layout-3col' : 'layout-2col';

  /* 列表列宽拖动：拖动分隔条调整文章列表列宽（280–560px），松手持久化。
     TASK-067 N9：拖动期间只更新本地拖拽态——此前 onMove 每像素调一次
     updateSettings（整包序列化 + set_setting IPC + 全 App 重渲染），一次拖动
     几十上百次写库；松手才一次性持久化。 */
  const startDragListWidth = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = listWidth;
    const onMove = (ev: MouseEvent) => {
      setDragWidth(Math.min(560, Math.max(280, startWidth + (ev.clientX - startX))));
    };
    const onUp = () => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
      setDragWidth((w) => {
        if (w != null) updateSettings({ listWidth: Math.round(w) });
        return null;
      });
    };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  };

  return (
    <>
      {/* 浮动窗口控制（Tauri 无边框窗口的原生控制） */}
      <div className="immersive-win-controls">
        <button className="win-btn" title="最小化" onClick={minimizeWindow}>—</button>
        <button className="win-btn" title="最大化/还原" onClick={toggleMaximizeWindow}>□</button>
        <button className="win-btn close" title="关闭" onClick={closeWindow}>✕</button>
      </div>

      <div className={`app-root ${gridClass}`} id="appRoot" style={{ '--list-width': `${effectiveListWidth}px` } as React.CSSProperties}>
        {/* 首屏加载骨架：dataLoading 期间不渲染真实 UI（避免空闪/测试数据闪现） */}
        {dataLoading ? (
          <div className="app-loading-splash">
            <div className="app-loading-logo"><img src="/logo.svg" alt="" draggable={false} /></div>
            <div className="app-loading-bar" />
          </div>
        ) : bootstrapError ? (
          <div className="app-loading-splash bootstrap-error">
            <div className="app-loading-logo"><img src="/logo.svg" alt="" draggable={false} /></div>
            <p className="bootstrap-error-text">启动失败：{bootstrapError}</p>
            <button
              className="bootstrap-error-retry"
              onClick={() => {
                void useAppStore.getState().retryBootstrap();
              }}
            >
              重试
            </button>
          </div>
        ) : (
          <>
            <Sidebar />
            <Timeline />
            {activeContentLayout === 'article' && (
              <div className="reader-resize-handle" onMouseDown={startDragListWidth} title="拖动调整列表宽度" />
            )}
            <Reader />
          </>
        )}
      </div>

      <PlayerBar />

      {/* 浮层 */}
      <SettingsModal />
      <SearchModal />
      <Lightbox />
      <NewCategoryModal />
      <AddFeedModal />
      <EditFeedModal />
      <RenameCategoryModal />
      <CloseAskDialog />
      {/* 全局右键菜单（替换 WebView2 默认菜单） */}
      <ContextMenuHost />

      {/* Toast：进场 = 挂载后下一帧切 visible（触发 transition）；
          退场 = store 标 leaving 后摘掉 visible，过渡完成再卸载。
          带操作按钮（action）的失败 toast 可一键重试。 */}
      <div className="toast-layer" aria-live="polite">
        {toasts.map((t) => (
          <ToastPill key={t.id} text={t.text} leaving={!!t.leaving} action={t.action} />
        ))}
      </div>
    </>
  );
}

/** 单条 toast：挂载后 rAF 切 visible 让 CSS transition 接管进场 */
function ToastPill({
  text,
  leaving,
  action,
}: {
  text: string;
  leaving: boolean;
  action?: { label: string; run: () => void };
}) {
  const [shown, setShown] = useState(false);
  useEffect(() => {
    const raf = requestAnimationFrame(() => setShown(true));
    return () => cancelAnimationFrame(raf);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return (
    <div className={`toast-pill ${shown && !leaving ? 'visible' : ''} ${action ? 'with-action' : ''}`}>
      <span className="toast-text">{text}</span>
      {action && <button className="toast-action-btn" onClick={action.run}>{action.label}</button>}
    </div>
  );
}
