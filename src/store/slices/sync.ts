import type { StateCreator } from 'zustand';
import { api, extractError, type RefreshSummary } from '../../lib/api';
import { openExternal } from '../../lib/external';
import { appStore } from '../internals';
import type { AppState } from '../types';

/** 同步 slice：手动刷新状态机 + GitHub 设备流登录（模块级常驻轮询）。
 *
 *  Pick 的键集即本 slice 的全部键；与其它 slice 两两不相交（合起来 = 原 useAppStore 全集）。
 */
export type SyncSlice = Pick<
  AppState,
  | 'syncStatus'
  | 'backgroundSyncing'
  | 'syncConnected'
  | 'githubFlow'
  | 'githubAccount'
  | 'githubLoggingIn'
  | 'triggerManualSync'
  | 'githubLoginStart'
  | 'githubLoginDisconnect'
>;

/* GitHub 设备流轮询的模块级定时器：常驻不受组件生命周期影响。
   poll 在成功/失败/被新登录覆盖时清掉；未授权期间按 interval 自续。 */
let githubPollTimer: ReturnType<typeof setTimeout> | null = null;

function scheduleGithubPoll(intervalSec: number) {
  if (githubPollTimer) clearTimeout(githubPollTimer);
  githubPollTimer = setTimeout(() => void doGithubPoll(), Math.max(3, intervalSec) * 1000);
}

function clearGithubPoll() {
  if (githubPollTimer) {
    clearTimeout(githubPollTimer);
    githubPollTimer = null;
  }
}

async function doGithubPoll() {
  githubPollTimer = null;
  try {
    const acc = await api.githubLoginPoll();
    if (acc) {
      clearGithubPoll();
      appStore().setState({ githubAccount: acc, githubFlow: null });
      appStore().getState().showToast(`已登录 GitHub：${acc.login}（Gist 同步已就绪）`);
    } else {
      const flow = appStore().getState().githubFlow;
      if (flow) scheduleGithubPoll(flow.interval);
    }
  } catch (e) {
    clearGithubPoll();
    appStore().setState({ githubFlow: null });
    appStore().getState().showToast(`GitHub 登录失败：${extractError(e)}`);
  }
}

export const createSyncSlice: StateCreator<AppState, [], [], SyncSlice> = (set, get) => ({
  syncStatus: 'synced',
  backgroundSyncing: false,
  syncConnected: false,
  githubFlow: null,
  githubAccount: null,
  githubLoggingIn: false,

  triggerManualSync: () => {
    if (get().dataMode === 'tauri') {
      set({ syncStatus: 'syncing' });
      /* 分步同步：feeds（订阅层）→ states（状态对账，只写状态不重拉列表）→
         refreshAllFeeds（内容抓取）。状态先落库、内容再抓取，最后只 reload 一次
         带出「最新内容 + 最新状态」，避免多次 reload 造成的列表闪动
         （「获取内容后再次同步状态导致闪动」的解耦）。 */
      void api
        .syncPhase('feeds')
        .catch(() => null) // 未连接（notConnected）→ 走纯直连刷新
        .then(async (feedsReport) => {
          if (feedsReport) {
            get().showToast('订阅同步完成，正在同步文章状态…');
            return api.syncPhase('states', true);
          }
          return null;
        })
        .then(async () => {
          /* states 阶段已把最新状态落库（is_read/is_starred），不在此 reload——
             等下方内容抓取完成后一次性 reload，避免「状态同步→闪动→内容→再闪动」 */
          return api.refreshAllFeeds();
        })
        .then((summary: RefreshSummary | null) => get().reloadFromBackend().then(() => summary))
        .then((summary: RefreshSummary | null) => {
          set({ syncStatus: 'synced' });
          if (summary && summary.failed_feeds > 0) {
            get().showToast(`已刷新，新增 ${summary.new_articles} 条，${summary.failed_feeds} 个源直连失败`);
          } else if (summary) {
            get().showToast(`已刷新，新增 ${summary.new_articles} 条`);
          } else {
            get().showToast('已刷新，无新文章');
          }
        })
        .catch((e: unknown) => {
          const msg = extractError(e);
          set({ syncStatus: 'error' });
          get().showToast(`刷新失败：${msg}`, { label: '重试', run: () => get().triggerManualSync() });
        });
      return;
    }
    set({ syncStatus: 'syncing' });
    get().showToast('刷新中…');
    setTimeout(() => {
      set({ syncStatus: 'synced' });
      get().showToast('已刷新');
    }, 1100);
  },

  /* ================= GitHub 设备流登录（store 级常驻轮询） ================= *
     GitHub 登录的发起/轮询放在 store 而非设置页组件：设备流等待授权
     可能跨数十秒，期间用户可能切走/关闭设置弹窗——组件卸载即清定时器，
     网页授权完成后软件再无反应（历史复现 bug）。模块级定时器不受组件
     生命周期影响，登录状态常驻 AppState。 */

  githubLoginStart: async () => {
    if (get().githubLoggingIn) return;
    set({ githubLoggingIn: true });
    try {
      let start: Awaited<ReturnType<typeof api.githubLoginStart>>;
      try {
        start = await api.githubLoginStart();
      } catch (first) {
        /* WebDAV 冲突：确认后带 force 重发（后端错误码 webdavConflict）。
           用结构化 code 判定而非 String(e)——Tauri 拒绝值是对象，
           String() 恒得 [object Object]，此前确认框永不弹出（P1-10） */
        const code = first && typeof first === 'object' ? (first as { code?: unknown }).code : undefined;
        const msg = extractError(first);
        if (code === 'webdavConflict' || msg.includes('WebDAV')) {
          if (!window.confirm(`${msg}

确定切换为 GitHub Gist 同步吗？`)) return;
          start = await api.githubLoginStart(true);
        } else {
          throw first;
        }
      }
      set({ githubFlow: start });
      await openExternal(start.verification_uri);
      get().showToast('已在浏览器打开授权页');
      scheduleGithubPoll(start.interval);
    } catch (e) {
      get().showToast(`发起登录失败：${extractError(e)}`);
    } finally {
      set({ githubLoggingIn: false });
    }
  },

  githubLoginDisconnect: async () => {
    set({ githubLoggingIn: true });
    try {
      await api.githubLoginDisconnect();
      clearGithubPoll();
      set({ githubAccount: null, githubFlow: null });
      set({ syncStatus: 'synced' });
      get().showToast('已断开 GitHub 登录');
    } catch (e) {
      get().showToast(`断开失败：${extractError(e)}`);
    } finally {
      set({ githubLoggingIn: false });
    }
  },
});
