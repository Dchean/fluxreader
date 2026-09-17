import type { StateCreator } from 'zustand';
import { api } from '../../lib/api';
import type { AppState } from '../types';

/** UI slice：弹层开关状态、关闭询问应答与 toast 队列。
 *
 *  Pick 的键集即本 slice 的全部键；与其它 slice 两两不相交（合起来 = 原 useAppStore 全集）。
 */
export type UiSlice = Pick<
  AppState,
  | 'settingsOpen'
  | 'settingsTab'
  | 'searchOpen'
  | 'closeAskVisible'
  | 'lightboxUrl'
  | 'newCategoryModalOpen'
  | 'addFeedModalOpen'
  | 'addFeedTargetCatId'
  | 'editFeedModalOpen'
  | 'editFeedTargetId'
  | 'renameCatModalOpen'
  | 'renameCatTargetId'
  | 'toasts'
  | 'openSettings'
  | 'closeSettings'
  | 'switchSettingsTab'
  | 'openSettingsTab'
  | 'openSearch'
  | 'closeSearch'
  | 'answerCloseAsk'
  | 'openLightbox'
  | 'closeLightbox'
  | 'openNewCategoryModal'
  | 'openAddFeedModal'
  | 'openEditFeedModal'
  | 'openRenameCatModal'
  | 'closeMiniModal'
  | 'showToast'
>;

let toastId = 0;

export const createUiSlice: StateCreator<AppState, [], [], UiSlice> = (set, get) => ({
  settingsOpen: false,
  settingsTab: 'general',
  searchOpen: false,
  closeAskVisible: false,
  lightboxUrl: null,
  newCategoryModalOpen: false,
  addFeedModalOpen: false,
  addFeedTargetCatId: '',
  editFeedModalOpen: false,
  editFeedTargetId: '',
  renameCatModalOpen: false,
  renameCatTargetId: '',

  toasts: [],

  /* ================= 弹层 ================= */

  openSettings: () => set({ settingsOpen: true }),
  closeSettings: () => set({ settingsOpen: false }),
  switchSettingsTab: (tab) => set({ settingsTab: tab }),
  openSettingsTab: (tab) => set({ settingsOpen: true, settingsTab: tab }),
  openSearch: () => set({ searchOpen: true }),
  closeSearch: () => set({ searchOpen: false }),
  answerCloseAsk: (action, remember) => {
    set({ closeAskVisible: false });
    /* remember 时同步设置镜像（真值由后端 resolve_close 落库，
       这里保证当前会话的设置页开关即时一致） */
    if (remember) {
      set((s) => ({ settings: { ...s.settings, closeToTray: action === 'tray', closePromptShown: true } }));
    }
    /* invoke 失败（DB 锁超时/IPC 错）时本地重试一次，仍失败 toast 提示
       （窗口可能没关——用户再点 ✕ 会走完整流程） */
    api.resolveClose(action, remember)
      .catch(() =>
        api.resolveClose(action, remember).catch(() =>
          get().showToast('关闭失败，请重试'),
        ),
      );
  },
  openLightbox: (url) => set({ lightboxUrl: url }),
  closeLightbox: () => set({ lightboxUrl: null }),
  openNewCategoryModal: () => set({ newCategoryModalOpen: true }),
  openAddFeedModal: (catId) => set({ addFeedModalOpen: true, addFeedTargetCatId: catId }),
  openEditFeedModal: (feedId) => set({ editFeedModalOpen: true, editFeedTargetId: feedId }),
  openRenameCatModal: (catId) => set({ renameCatModalOpen: true, renameCatTargetId: catId }),
  closeMiniModal: (which) =>
    set(
      which === 'newCategory' ? { newCategoryModalOpen: false }
      : which === 'addFeed' ? { addFeedModalOpen: false }
      : which === 'editFeed' ? { editFeedModalOpen: false }
      : { renameCatModalOpen: false },
    ),

  showToast: (text, action) => {
    const id = ++toastId;
    set((s) => ({
      /* 上限 4 条：错误循环（如后台刷新连续失败）不再无限堆叠；保留最新 */
      toasts: [...s.toasts, { id, text, action }].slice(-4),
    }));
    /* 两段式生命周期：2200ms 后先标 leaving（CSS 退场过渡），200ms 过渡完成再卸载；
       带操作按钮时延长停留（留出点重试的时间） */
    const stay = action ? 4200 : 2200;
    setTimeout(() => {
      set((s) => ({ toasts: s.toasts.map((t) => (t.id === id ? { ...t, leaving: true } : t)) }));
      setTimeout(() => {
        set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
      }, 200);
    }, stay);
  },
});
