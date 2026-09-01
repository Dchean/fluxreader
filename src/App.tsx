import { useEffect, useState } from 'react';
import { useAppStore, selectVisibleEntries } from './store';
import { Sidebar } from './components/Sidebar';
import { Timeline } from './components/Timeline';
import { Reader } from './components/Reader';
import { PlayerBar } from './components/PlayerBar';
import { SettingsModal } from './components/SettingsModal';
import { SearchModal, Lightbox, NewCategoryModal, AddFeedModal, EditFeedModal, RenameCategoryModal, CloseAskDialog } from './components/Overlays';
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
  const toasts = useAppStore((s) => s.toasts);

  /* ---------- 数据源装载：Tauri 环境从 SQLite 拉全量；浏览器保持 mock ---------- */
  useEffect(() => {
    /* 设置恢复先于数据装载：主题/视图等在首帧就位，避免闪默认值 */
    void useAppStore.getState().bootstrapSettings().then(() => {
      useAppStore.getState().bootstrapFromBackend();
    });
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
        const { listen } = await import('@tauri-apps/api/event');
        unlisten = await listen('close-ask', () => {
          useAppStore.setState({ closeAskVisible: true });
        });
      } catch {
        /* 忽略：监听失败时按默认行为关闭 */
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
        else if (s.settingsOpen) s.closeSettings();
        else if (s.lightboxUrl) s.closeLightbox();
        else if (s.activeArticleId) s.clearReaderSelection();
        return;
      }
      if (inInput) return;

      /* Space：播放器激活时播放/暂停（快捷键表承诺） */
      if (e.key === ' ' && s.player.isActive) {
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

  return (
    <>
      {/* 浮动窗口控制（Tauri 无边框窗口的原生控制） */}
      <div className="immersive-win-controls">
        <button className="win-btn" title="最小化" onClick={minimizeWindow}>—</button>
        <button className="win-btn" title="最大化/还原" onClick={toggleMaximizeWindow}>□</button>
        <button className="win-btn close" title="关闭" onClick={closeWindow}>✕</button>
      </div>

      <div className={`app-root ${gridClass}`} id="appRoot">
        <Sidebar />
        <Timeline />
        <Reader />
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
