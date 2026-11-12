import { useEffect, useState } from 'react';
import { useAppStore, selectVisibleEntries, readBackAppliedAppSettings, runAppSettingsApplyBoundary } from './store';
import { Sidebar } from './components/Sidebar';
import { Timeline } from './components/Timeline';
import { Reader } from './components/Reader';
import { PlayerBar } from './components/PlayerBar';
import { SettingsModal } from './components/SettingsModal';
import { SearchModal, Lightbox, NewCategoryModal, AddFeedModal, EditFeedModal, RenameCategoryModal, CloseAskDialog } from './components/Overlays';
import { ErrorCenterDialog } from './components/ErrorCenter';
import { ContextMenuHost } from './components/ContextMenu';
import { ErrorBoundary } from './components/ErrorBoundary';
import { useTauriEvent } from './components/useTauriEvent';
import { ModalOverlay } from './components/primitives';
import { extractError } from './lib/api';
import { anyOverlayOpen, isInteractiveSpaceTarget, shouldYieldToOverlay } from './components/shortcutYield';
import { jkLayoutAllowed, jkNextIndex } from './lib/jkNavigation';
import type { SyncQueueStats } from './lib/api';
import type { BackendSyncActivityEvent } from './store/types';
import { localDayBounds } from './store/readingSession';
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
  const libraryRestoreBusy = useAppStore((s) => s.libraryRestoreBusy);
  const libraryGeneration = useAppStore((s) => s.libraryGeneration);
  const portablePhase = useAppStore((s) => s.portableBackup.busy);

  useEffect(() => {
    // 在任何全局/浮层快捷键之前处理。Tab 仍交给恢复对话框的 focus trap。
    const blockDuringRestore = (event: KeyboardEvent) => {
      if (!useAppStore.getState().libraryRestoreBusy || event.key === 'Tab') return;
      event.preventDefault(); event.stopImmediatePropagation();
    };
    window.addEventListener('keydown', blockDuringRestore, true);
    return () => window.removeEventListener('keydown', blockDuringRestore, true);
  }, []);

  // 日界刷新只更新当前会话属性/计数；下一次显式导航才捕获新的“今天”成员。
  useEffect(() => {
    let day = localDayBounds().dayStartMs;
    let timer: ReturnType<typeof setTimeout>;
    const checkDay = () => {
      const next = localDayBounds();
      if (next.dayStartMs !== day) {
        day = next.dayStartMs;
        const state = useAppStore.getState();
        if (state.dataMode === 'tauri' && !state.dataLoading) {
          void state.reloadFromBackend({ keepReadingPosition: true }).catch(() => { /* 刷新自身报告失败 */ });
        }
      }
      clearTimeout(timer);
      timer = setTimeout(checkDay, Math.max(1, next.dayEndMs - Date.now()));
    };
    checkDay();
    window.addEventListener('focus', checkDay);
    document.addEventListener('visibilitychange', checkDay);
    return () => {
      clearTimeout(timer);
      window.removeEventListener('focus', checkDay);
      document.removeEventListener('visibilitychange', checkDay);
    };
  }, []);

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
    /* 设置恢复与来源/计数元数据并行；完整成员查询等待启动筛选接受，随后原子落地。 */    void useAppStore.getState().bootstrapFromBackend();
    if ('__TAURI_INTERNALS__' in window) {
      void useAppStore.getState().refreshLocalSyncStatus().catch((error) => {
        useAppStore.getState().showToast(`WebDAV 状态读取失败：${extractError(error)}`);
      });
    }
  }, []);

  /* ---------- 后台刷新调度器事件：新文章到达即重载列表 + toast ---------- */
  useTauriEvent<{
    new_articles?: number;
    failed_feeds?: number;
    source?: 'local-sync' | 'portable-restore';
    /* OPT-007（F11）：后台同步的数据变化事件按实际变更集驱动——纯读藏
       对账（merged_states>0、无新文章）也会到达本监听器；两字段仅为
       可诊断性随 payload 下发，落地动作与两条既有通道一致。 */
    pulled_entries?: number;
    merged_states?: number;
  }>('feeds-updated', (e) => {
    // restore 的全量重建由提交调用者单独拥有；重复事件不能再次清空新会话。
    if (e.source === 'portable-restore' || useAppStore.getState().libraryRestoreBusy) return;
    const { new_articles, failed_feeds } = e;
    /* TASK-098（与 F5 同口径）：失败提示由 reloadFromBackend 的 toast 给出，这里吞掉 toast 后的重抛 */
    /* TASK-111②：后台刷新路径携带保位请求——落地时发出保位信号，
       Timeline 消费顶条锚回位（审计「后台刷新保留当前阅读位置」）。
       OPT-007：纯对账变化（new_articles=0）同样走这条保位 reload——
       列表/计数/Reader 跟随更新，锚点保留；空变化不在 Rust 侧发事件。 */
    void useAppStore.getState().reloadFromBackend({ keepReadingPosition: true }).catch(() => { /* 失败已可见（reloadFromBackend 内 toast） */ });
    if ((new_articles ?? 0) > 0) {
      useAppStore.getState().showToast(`后台刷新：新文章 ${new_articles} 篇`);
    } else if ((failed_feeds ?? 0) > 0) {
      useAppStore.getState().showToast(`后台刷新：${failed_feeds} 个源抓取失败`);
    }
  });

  useTauriEvent<{ kind: 'state' | 'body' | 'restore'; ok: boolean }>('local-sync-updated', (event) => {
    if (useAppStore.getState().libraryRestoreBusy || event.kind === 'restore') return;
    void useAppStore.getState().refreshLocalSyncStatus().catch(() => {});
    if (event.kind === 'state' && event.ok) {
      // 普通同步刷新设置基线，不把启动筛选应用到正在阅读的会话。
      void runAppSettingsApplyBoundary(() => readBackAppliedAppSettings({ applyViewFilters: false })).catch((error) => {
        useAppStore.getState().showToast(`状态同步已完成，但设置回读失败：${extractError(error)}`);
      });
    }
  });

  useTauriEvent<BackendSyncActivityEvent>('sync-activity', (event) => {
    useAppStore.getState().applyBackendSyncActivity(event);
  });

  /* ---------- TASK-124（审计 P2-6①）：同步队列变化事件——四态展示接入真实链路 ----------
     Rust 在本地入队事务提交（set_read/set_starred/set_read_bulk/mark_all_read）、
     即时推送与 states 推送段确认·失败、认证/网络阻塞后发 sync-queue-changed
     （payload = 队列生命周期分类统计）。此前队列统计只随 reloadFromBackend
     刷新——本地标读入队后 pill 仍显示「后端已同步」（探针 P8 本体）。
     监听器只调 applySyncQueueChanged（store 动作）：写队列分类字段，并按最新
     统计设置或撤销手动同步 states 阶段的远端写回确认；不触碰 syncConnected，
     也不以队列清空结束整轮 syncing。未配置同步时 Rust 不发事件，pill 保持
     「本地模式」不误报。 */
  useTauriEvent<SyncQueueStats>('sync-queue-changed', (e) => {
    useAppStore.getState().applySyncQueueChanged(e);
  });

  /* 即时状态推送的基础设施错误（计划构建/失败标记/出队写库失败）不能只进
     Rust 日志：转换为带重试入口的可见错误，普通推送失败仍由队列 last_error
     负责，不在这里重复刷屏。 */
  useTauriEvent<{ message: string }>('sync-error', (e) => {
    const message = e?.message?.trim() || '状态同步失败';
    useAppStore.getState().showToast(message, {
      label: '重试',
      key: 'sync:instant-push',
      run: () => { void useAppStore.getState().triggerManualSync(); },
    });
  });

  /* ---------- OPT-009 B3：订阅 outbox 状态变化事件——持久回执接入真实链路 ----------
     Rust 在订阅写回入队（新增/编辑/删除/分类操作/OPML/首连推送，事务提交后）
     与执行器确认出队/失败累计/local_only 标记后发 `subscription-outbox-changed`
     （payload = 当前账号 `{pending, failed, local_only, last_error}`，账号限定）。
     设置页同步摘要卡与能力卡经 store 字段即时跟随，不依赖用户手动刷新。
     未配置同步时 Rust 不发事件（不制造噪音）；监听器只写回执字段，
     不触碰队列统计与连接态。 */
  useTauriEvent<{ pending: number; failed: number; local_only: number; last_error: string | null }>(
    'subscription-outbox-changed',
    (e) => {
      useAppStore.getState().applySubscriptionOutboxChanged(e);
    },
  );

  /* ---------- SMTC 系统媒体键回调：媒体键/音量浮层控制播放 ---------- */
  useTauriEvent<string>('player-media', (action) => {
    if (useAppStore.getState().libraryRestoreBusy) return;
    /* OPT-015：媒体键语义（play/pause 目标态幂等、toggle 才切换、未激活
       不启动、stop 关闭）单点在 store.applyMediaAction；这里只收窄
       payload，不再内联播放判定——此前 play/pause/toggle 全部
       togglePlayerPlay，播放中按「播放」会反被暂停。 */
    const st = useAppStore.getState();
    if (action === 'play' || action === 'pause' || action === 'toggle' || action === 'stop') {
      st.applyMediaAction(action);
    }
  });

  /* ---------- 首次关闭询问：Rust CloseRequested 判断未问过 → 弹窗 ---------- */
  useTauriEvent('close-ask', () => {
    useAppStore.setState({ closeAskVisible: true });
    /* 回 ack：让 Rust 侧知道前端已接管（10s 无 ack Rust 会兜底隐藏，
       避免窗口"关不掉"；ack 前提是弹窗真的弹出来了） */
    void import('@tauri-apps/api/event').then(({ emit }) => emit('close-ask-ack', 'frontend-ready'));
  });

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
      if (e.defaultPrevented) return;
      const s = useAppStore.getState();
      const target = e.target as HTMLElement;
      const inInput = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable;

      if (e.ctrlKey && !e.altKey && !e.metaKey && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        if (s.searchOpen) s.closeSearch(); else s.openSearch();
        return;
      }
      if (e.ctrlKey && !e.altKey && !e.metaKey && e.key === ',') {
        e.preventDefault();
        if (s.settingsOpen) s.closeSettings(); else s.openSettings();
        return;
      }
      if (e.key === 'Escape') {
        /* IME 守卫（审计卡2-1，口径同 Overlays 命令面板 Enter 的 isComposing 判）：
           输入法合成中的 Escape（nativeEvent.isComposing，或 Chromium/WebView2
           合成期的 keyCode 229）语义是「取消选词」，不得关闭浮层。本分支位于
           inInput 判断之前，输入框内合成同样会到达这里，故不能依赖焦点守卫。 */
        // Note: 守卫口径与「为何不全局拦截」 — 见 .agents/notes/implemented/feature/2026-11-02-前端交互正确性与感知体验.md
        if (e.isComposing || e.keyCode === 229) return;
        /* 浮层按既有 Esc 链依次关闭。CloseAskDialog 与其他 ModalOverlay 同为
           z-index 150，本身不监听 Esc；本分支先取消关闭询问，再关闭搜索/迷你
           弹窗/错误中心/设置，最后处理灯箱、全屏播放器与阅读器。
           Esc 语义 = 仅取消本次关闭询问：不最小化、不退出、不写 remember；
           只有显式按钮才提交关闭动作。 */
        if (s.closeAskVisible) s.dismissCloseAsk();
        else if (s.searchOpen) s.closeSearch();
        else if (s.newCategoryModalOpen) s.closeMiniModal('newCategory');
        else if (s.addFeedModalOpen) s.closeMiniModal('addFeed');
        else if (s.editFeedModalOpen) s.closeMiniModal('editFeed');
        else if (s.renameCatModalOpen) s.closeMiniModal('renameCat');
        else if (s.errorCenterOpen) s.closeErrorCenter(); // OPT-017E：错误中心面板
        else if (s.settingsOpen) s.closeSettings();
        else if (s.lightboxUrl) s.closeLightbox();
        else if (s.playerExpanded && s.player.isActive) s.togglePlayerExpanded(); // Full Player 大浮层
        else if (s.activeArticleId) s.clearReaderSelection();
        return;
      }
      // 已声明的组合键在上方消费；其它修饰组合不能降级为背后的单键动作。
      if (inInput || e.ctrlKey || e.metaKey || e.altKey) return;

      /* P3[F2]（REQ-104）：任一浮层打开时，**单键快捷键**（S/M/J/K）一律让路。
         此前只有 Space 判断了 defaultPrevented，S/M/J/K 不看浮层状态——设置页、
         搜索框、各类弹窗打开时，焦点若不在输入框上，按 S/M 会作用到**浮层背后的
         当前文章**（改了它的收藏/已读却看不见），J/K 还会在背后切换文章。
         OPT-012：补上「关闭确认框」（closeAskVisible）——它此前漏出浮层清单，
         确认框打开时单键同样会作用到背后文章；Esc 仍走上方专门分支（不改语义）。
         Ctrl+K / Ctrl+, / Escape 属浮层自身操作，在此判断之前已处理，不受影响。
         判据已抽成纯函数 shouldYieldToOverlay（src/components/shortcutYield.ts），
         浮层集合由同模块的 anyOverlayOpen 按 OVERLAY_SOURCES 清单求值，
         两者均由回归网直接断言——此前内联在这里，属「改了行为却无法断言」的覆盖缺口
         （审查 TASK-081-F2 登记的遗留）。 */
      const overlayOpen = anyOverlayOpen({
        searchOpen: s.searchOpen,
        contextMenuOpen: s.contextMenuOpen,
        settingsOpen: s.settingsOpen,
        newCategoryModalOpen: s.newCategoryModalOpen,
        addFeedModalOpen: s.addFeedModalOpen,
        editFeedModalOpen: s.editFeedModalOpen,
        renameCatModalOpen: s.renameCatModalOpen,
        lightboxUrl: s.lightboxUrl,
        playerExpanded: s.playerExpanded,
        playerActive: s.player.isActive,
        closeAskVisible: s.closeAskVisible,
        errorCenterOpen: s.errorCenterOpen, // OPT-017E：错误中心面板
      });
      if (shouldYieldToOverlay(overlayOpen, e.key, e.ctrlKey || e.metaKey || e.altKey) === 'yield') {
        return;
      }

      /* Space：播放器激活时播放/暂停（快捷键表承诺）。
         原生交互控件（按钮/链接/表单/菜单项等）拥有自己的 Space 语义，必须
         整体让路；卡片虽常带 role=button，但不在此清单内，仍交给播放器全局
         切换。defaultPrevented 继续兜住更具体的自定义控件处理器。
         只在此分支判断，不动其它快捷键的既有语义。 */
      if (e.key === ' ' && s.player.isActive) {
        if (e.defaultPrevented || isInteractiveSpaceTarget(target)) return;
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
      /* J/K 键盘流：五布局统一选中；文章分区同时打开正文。activeArticleId 变化后由
         Timeline 负责目标卡入窗 + DOM focus + aria-current；门控与推进判定
         仍收口在纯函数 jkLayoutAllowed/jkNextIndex，回归网直接断言真值表、
         五布局 store 模拟与真实组件焦点。 */
      if (e.key === 'j' || e.key === 'k') {
        if (!jkLayoutAllowed(s.activeContentLayout)) return;
        const items = selectVisibleEntries(s);
        const curIdx = items.findIndex((a) => a.id === s.activeArticleId);
        const nextIdx = jkNextIndex(items.length, curIdx, e.key === 'j');
        if (nextIdx >= 0 && items[nextIdx]) s.selectArticle(items[nextIdx].id);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const gridClass = activeContentLayout === 'article' ? 'layout-3col' : 'layout-2col';

  /* 列表列宽拖动：拖动分隔条调整文章列表列宽（280–420px），松手持久化。
     视口过窄时 CSS 还会按 37vw 收缩，保证阅读列不被固定列宽挤没。
     TASK-067 N9：拖动期间只更新本地拖拽态——此前 onMove 每像素调一次
     updateSettings（整包序列化 + set_setting IPC + 全 App 重渲染），一次拖动
     几十上百次写库；松手才一次性持久化。 */
  const startDragListWidth = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = Math.min(420, listWidth);
    const onMove = (ev: MouseEvent) => {
      setDragWidth(Math.min(420, Math.max(280, startWidth + (ev.clientX - startX))));
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
        <button className="win-btn close" title="关闭" disabled={libraryRestoreBusy} onClick={closeWindow}>✕</button>
      </div>

      {/* 产品树错误边界：主内容 + 浮层 + toast 崩溃时出全屏兜底态；
          窗口控制留在边界外——无边框窗口在崩溃态下仍必须能最小化/关闭。
          Note: 归因与分层取舍见 .agents/notes/implemented/architecture/2026-11-01-产品树错误边界放顶层.md */}
      <ErrorBoundary>
        <div inert={libraryRestoreBusy} style={{ display: 'contents' }}>
        <div className={`app-root ${gridClass}`} id="appRoot" style={{ '--list-width': `${effectiveListWidth}px` } as React.CSSProperties}>
          {/* 首屏加载骨架：dataLoading 期间不渲染真实 UI（避免空闪/测试数据闪现） */}
          {dataLoading ? (
            <div className="app-loading-splash" role="status" aria-label="加载中">
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
              {activeContentLayout === 'article' && <Reader />}
            </>
          )}
        </div>

        <PlayerBar />

        {/* 浮层 */}
        <SettingsModal key={`settings:${libraryGeneration}`} />
        <SearchModal key={`search:${libraryGeneration}`} />
        <Lightbox />
        <NewCategoryModal />
        <AddFeedModal />
        <EditFeedModal />
        <RenameCategoryModal />
        {/* OPT-017E：错误中心面板（侧栏入口打开；Esc/✕/遮罩可关） */}
        <ErrorCenterDialog />
        <CloseAskDialog />
        {/* 全局右键菜单（替换 WebView2 默认菜单） */}
        <ContextMenuHost />
        </div>
        {libraryRestoreBusy && <div style={{ position: 'fixed', inset: 0, zIndex: 10000 }}>
          <ModalOverlay open onClose={() => {}} ariaLabel="正在恢复资料库" contentWidth={420}>
            <div className="mini-dialog" role="status" aria-live="polite" aria-busy="true">
              <h3>{portablePhase === 'refresh' ? '恢复已完成，正在刷新界面…' : '正在恢复资料库…'}</h3>
              <p>请稍候，完成前暂时不能编辑资料库。</p>
            </div>
          </ModalOverlay>
        </div>}

        {/* Toast：进场 = 挂载后下一帧切 visible（触发 transition）；
            退场 = store 标 leaving 后摘掉 visible，过渡完成再卸载。
            带操作按钮（action）的失败 toast 可一键重试——经 store.retryError 派发，
            与错误中心面板同一语义（重试即消费对应条目，失败路径重报写回）。 */}
        <div className="toast-layer" aria-live="polite">
          {toasts.map((t) => (
            <ToastPill key={t.id} id={t.id} text={t.text} leaving={!!t.leaving} action={t.action} />
          ))}
        </div>
      </ErrorBoundary>
    </>
  );
}

/** 单条 toast：挂载后 rAF 切 visible 让 CSS transition 接管进场 */
function ToastPill({
  id,
  text,
  leaving,
  action,
}: {
  id: number;
  text: string;
  leaving: boolean;
  action?: { label: string; run: () => void };
}) {
  const [shown, setShown] = useState(false);
  const retryError = useAppStore((s) => s.retryError);
  useEffect(() => {
    const raf = requestAnimationFrame(() => setShown(true));
    return () => cancelAnimationFrame(raf);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return (
    <div className={`toast-pill ${shown && !leaving ? 'visible' : ''} ${action ? 'with-action' : ''}`}>
      <span className="toast-text">{text}</span>
      {action && <button className="toast-action-btn" onClick={() => retryError(id)}>{action.label}</button>}
    </div>
  );
}
