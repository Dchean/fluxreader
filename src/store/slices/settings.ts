import type { StateCreator } from 'zustand';
import { api } from '../../lib/api';
import type { ViewFilterType } from '../../types';
import type { AppState, SettingsState } from '../types';

/** 设置 slice：设置项初值、乐观更新与启动恢复。
 *
 *  Pick 的键集即本 slice 的全部键；与其它 slice 两两不相交（合起来 = 原 useAppStore 全集）。
 */
export type SettingsSlice = Pick<
  AppState,
  | 'settings'
  | 'updateSettings'
  | 'bootstrapSettings'
>;

export const createSettingsSlice: StateCreator<AppState, [], [], SettingsSlice> = (set, get) => ({
  settings: {
    autoRefresh: true,
    refreshInterval: 30,
    fetchConcurrency: 4,
    markReadOnOpen: true,
    markReadOnScrollBottom: false,
    markReadOnScrollOut: false,
    autoStart: false,
    startupView: 'unread',
    hideReadOnStartup: true,
    themeMode: 'dark',
    palette: 'blue',
    fontFamily: '"Segoe UI Variable Text", "Segoe UI", "Microsoft YaHei UI", "Microsoft YaHei", system-ui, sans-serif',
    fontSize: 16,
    lineHeight: 180,
    maxWidth: 860,
    listWidth: 320,
    showReadTime: true,
    defaultOpenMode: 'rss',
    smartDedup: false,
    closeToTray: true,
    closePromptShown: false,
    notifyOnNewArticles: false,
    autoSync: true,
    syncMode: 'direct',
  },

  updateSettings: (partial) => {
    set((s) => ({ settings: { ...s.settings, ...partial } }));
    /* 持久化到后端（单键 JSON；浏览器 mock 环境无 IPC 跳过） */
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      void api.setSetting('app_settings', JSON.stringify(get().settings));
    }
  },

  /** 启动时从后端恢复设置；并应用 startupView / hideReadOnStartup */
  bootstrapSettings: async () => {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
    try {
      const raw = await api.getSetting('app_settings');
      if (!raw) return;
      const saved = JSON.parse(raw) as Partial<SettingsState>;
      /* 逐键合并（未来新增设置项自动落默认值）；类型不符的丢弃 */
      const merged: SettingsState = { ...get().settings };
      const target = merged as unknown as Record<string, unknown>;
      for (const [k, v] of Object.entries(saved)) {
        if (k in merged && typeof v === typeof target[k]) {
          target[k] = v;
        }
      }
      /* maxWidth 旧默认迁移：v0.10.x 默认 760，现已提升为 860。若用户从未
         主动改过（值恰等于旧默认 760），升级到新默认；主动设过的值保留。 */
      if (merged.maxWidth === 760) {
        merged.maxWidth = 860;
      }
      /* startupView：启动默认视图（未读/全部/今天/收藏） */
      const view = saved.startupView;
      const validView = view === 'all' || view === 'today' || view === 'unread' || view === 'starred';
      set((s) => ({
        settings: merged,
        activeViewFilter: validView ? (view as ViewFilterType) : s.activeViewFilter,
        /* hideReadOnStartup：启动时时间流默认筛选（unread=隐藏已读 / all=显示全部） */
        timelineFilter: saved.hideReadOnStartup === false ? 'all' : 'unread',
      }));
    } catch (e) {
      console.error('restore settings failed:', e);
    }
  },
});
