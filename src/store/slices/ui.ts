import type { StateCreator } from 'zustand';
import { api } from '../../lib/api';
import type { AppErrorEntry, AppState } from '../types';

/** UI slice：弹层开关状态、关闭询问应答与 toast 队列 + 错误中心（OPT-017E）。
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
  | 'lightboxEntryId'
  | 'newCategoryModalOpen'
  | 'addFeedModalOpen'
  | 'addFeedTargetCatId'
  | 'editFeedModalOpen'
  | 'editFeedTargetId'
  | 'renameCatModalOpen'
  | 'renameCatTargetId'
  | 'toasts'
  | 'errors'
  | 'errorCenterOpen'
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
  | 'retryError'
  | 'dismissError'
  | 'clearErrors'
  | 'openErrorCenter'
  | 'closeErrorCenter'
>;

/* ============================================================
   OPT-017E：错误中心（重试入口持久可达 + 有界内存）

   问题：toast 层上限 4 条（`.slice(-4)`）且带动作项 4.2s 后也退场——后台连续
   失败时，唯一的重试入口会被静默挤掉（父卡 OPT-017：「toast 上限不能悄悄
   挤掉唯一重试入口」，也不能用无限数组堆回调规避）。

   决策：`showToast(text, action)` 带动作时**同时**写入错误中心——一个有界列表
   （上限 20 条）、按 key 合并最新、TTL 24h 惰性清理；面板经侧栏入口打开，
   逐条可重试/忽略/全部清除。普通提示不进错误中心、形态与时长不变。
   去重键（R1 返工）：**显式操作实体 key 优先**（`action.key`，命名 `域:操作:实体`
   如 `feed:refresh:10`；全部带重试动作的调用点都已迁移），缺省回落文案——同文案
   不同实体不得合并成一条（否则多源同类失败只能重试最新一个）；迁移完整性由域
   回归 (e9) 的源级守卫钉住。
   取舍与替代方案见 .agents/notes/implemented/feature/2026-10-16-重试入口持久可达与错误中心.md。 */
export const ERROR_CENTER_MAX = 20;
export const ERROR_CENTER_TTL_MS = 24 * 60 * 60 * 1000;

/** 纯函数：同 key 合并（替换为最新条目、落到最新位）+ 超上限从最旧逐出。
 *  不改入参数组（返回新数组）——store 侧 set 用它计算 next，测试可直接做真值表。 */
export function upsertErrorEntry(
  list: AppErrorEntry[],
  entry: AppErrorEntry,
  max: number = ERROR_CENTER_MAX,
): AppErrorEntry[] {
  const next = [...list.filter((e) => e.key !== entry.key), entry];
  return next.length > max ? next.slice(next.length - max) : next;
}

/** 纯函数：惰性清理超过 TTL 未更新的条目（生命周期上限）。 */
export function pruneErrorEntries(
  list: AppErrorEntry[],
  now: number,
  ttl: number = ERROR_CENTER_TTL_MS,
): AppErrorEntry[] {
  return list.filter((e) => now - e.at < ttl);
}

let toastId = 0;

/* ============================================================
   OPT-018：toast 退场定时器登记（组件回归稳定性）

   showToast 的两段 setTimeout（停留 2200/4200ms + 退场 200ms）此前没有句柄：
   用例结束后它们仍会触发 store.set，下一用例挂载 ToastPill 时这条更新落在
   act 之外，React 19 报「not wrapped in act」并把失败随机落到后续用例。
   这里只登记句柄并提供 clearToastTimers（测试夹具复位时清空）；生产路径的
   停留时长、退场过渡与错误中心语义均不变。
   ============================================================ */
type ToastTimers = {
  leave?: ReturnType<typeof setTimeout>;
  remove?: ReturnType<typeof setTimeout>;
};
const toastTimers = new Map<number, ToastTimers>();

/** 清空全部 pending 的 toast 定时器（仅测试夹具使用；生产代码不调用）。 */
export function clearToastTimers() {
  for (const { leave, remove } of toastTimers.values()) {
    if (leave !== undefined) clearTimeout(leave);
    if (remove !== undefined) clearTimeout(remove);
  }
  toastTimers.clear();
}

export const createUiSlice: StateCreator<AppState, [], [], UiSlice> = (set, get) => ({
  settingsOpen: false,
  settingsTab: 'general',
  searchOpen: false,
  closeAskVisible: false,
  lightboxUrl: null,
  lightboxEntryId: null,
  newCategoryModalOpen: false,
  addFeedModalOpen: false,
  addFeedTargetCatId: '',
  editFeedModalOpen: false,
  editFeedTargetId: '',
  renameCatModalOpen: false,
  renameCatTargetId: '',

  toasts: [],
  errors: [],
  errorCenterOpen: false,

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
  openLightbox: (url, entryId) => set({ lightboxUrl: url, lightboxEntryId: entryId ?? null }),
  closeLightbox: () => set({ lightboxUrl: null, lightboxEntryId: null }),
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
    const now = Date.now();
    set((s) => {
      /* 上限 4 条：错误循环（如后台刷新连续失败）不再无限堆叠；保留最新。
         带动作的错误另有错误中心兜底（下方 errors 写入），toast 被挤掉不等于入口消失。 */
      const toasts = [...s.toasts, { id, text, action }].slice(-4);
      if (!action) return { toasts };
      /* OPT-017E：带重试动作的失败同时写入错误中心——有界（≤20）+ 同 key
         合并最新 + 惰性 TTL 清理；id 复用 toast 计数（单调唯一）。
         R1 返工：去重键 = 显式操作实体 key 优先、缺省回落文案——同文案不同
         实体（多源同类失败）各自成条、各自可重试。 */
      const key = action.key ?? text;
      const kept = pruneErrorEntries(s.errors, now);
      const prev = kept.find((e) => e.key === key);
      const entry: AppErrorEntry = {
        id,
        key,
        text,
        action,
        count: (prev?.count ?? 0) + 1,
        at: now,
      };
      return { toasts, errors: upsertErrorEntry(kept, entry) };
    });
    /* 两段式生命周期：2200ms 后先标 leaving（CSS 退场过渡），200ms 过渡完成再卸载；
       带操作按钮时延长停留（留出点重试的时间）。句柄登记在 toastTimers，
       以便测试夹具在用例之间清除 pending 定时器（OPT-018）。 */
    const stay = action ? 4200 : 2200;
    const timers: ToastTimers = {};
    toastTimers.set(id, timers);
    timers.leave = setTimeout(() => {
      set((s) => ({ toasts: s.toasts.map((t) => (t.id === id ? { ...t, leaving: true } : t)) }));
      timers.remove = setTimeout(() => {
        set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
        toastTimers.delete(id);
      }, 200);
    }, stay);
  },

  /* ================= 错误中心（OPT-017E） ================= */

  /* 派发重试并消费条目：条目是「一次失败」的快照，用户已重试即消费（两个表示面
     同一语义——错误中心面板与瞬时 toast 的重试按钮都经本动作）。
     找不到对应条目时回落到同 id 的带动作 toast（条目可能已被同 key 合并/逐出，
     而旧 toast 仍在屏上）；两者皆无 → no-op。
     消费（按 key 清条目）**先于**执行：失败路径在动作内同步重报时，新条目在
     消费之后写入 → 保留（失败不丢入口）；成功路径不重报 → 条目保持清除。
     同步抛错的失败不会触发重报，此时恢复消费前的条目快照（失败不丢入口）。 */
  retryError: (id) => {
    const s = get();
    const entry = s.errors.find((e) => e.id === id);
    const toast = entry ? undefined : s.toasts.find((t) => t.id === id && t.action);
    const action = entry?.action ?? toast?.action;
    const key = entry?.key ?? toast?.action?.key ?? toast?.text;
    if (!action || key === undefined) return;
    const before = s.errors;
    set({ errors: before.filter((e) => e.key !== key) });
    try {
      action.run();
    } catch {
      set({ errors: before });
    }
  },
  dismissError: (id) => set((s) => ({ errors: s.errors.filter((e) => e.id !== id) })),
  clearErrors: () => set({ errors: [] }),
  openErrorCenter: () => set({ errorCenterOpen: true }),
  closeErrorCenter: () => set({ errorCenterOpen: false }),
});
