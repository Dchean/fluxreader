import type { StateCreator } from 'zustand';
import { api, extractError, type RefreshSummary, type SubscriptionOutboxStats } from '../../lib/api';
import { openExternal } from '../../lib/external';
import { appStore } from '../internals';
import { syncFailureMessage } from '../syncErrors';
import type {
  AppState,
  ManualSyncFailure,
  ManualSyncStage,
  SyncActivityOwner,
  SyncActivityUpdate,
} from '../types';

/** 同步 slice：手动刷新状态机 + GitHub 设备流登录（模块级常驻轮询）。
 *
 *  Pick 的键集即本 slice 的全部键；与其它 slice 两两不相交（合起来 = 原 useAppStore 全集）。
 */
export type SyncSlice = Pick<
  AppState,
  | 'syncStatus'
  | 'syncActivityOwner'
  | 'syncActivityToken'
  | 'syncStage'
  | 'syncStartedAt'
  | 'syncRemoteWriteConfirmed'
  | 'syncFailure'
  | 'backgroundSyncing'
  | 'syncConnected'
  | 'syncWaiting'
  | 'syncFailed'
  | 'syncQueueLastError'
  | 'subOutboxStats'
  | 'subscriptionCapabilities'
  | 'githubFlow'
  | 'githubAccount'
  | 'githubLoggingIn'
  | 'beginSyncActivity'
  | 'updateSyncActivity'
  | 'finishSyncActivity'
  | 'triggerManualSync'
  | 'applySyncQueueChanged'
  | 'applySubscriptionOutboxChanged'
  | 'refreshSubscriptionOutboxStats'
  | 'loadSubscriptionCapabilities'
  | 'githubLoginStart'
  | 'githubLoginDisconnect'
>;

/* GitHub 设备流轮询的模块级定时器：常驻不受组件生命周期影响。
   poll 在成功/失败/被新登录覆盖时清掉；未授权期间按 interval 自续。 */
let githubPollTimer: ReturnType<typeof setTimeout> | null = null;
/* OPT-022：手动同步是 store 级单飞任务。组件卸载/切页不影响在途 Promise；
   重复入口复用同一 Promise，避免两轮完整同步互相覆盖终态。 */
let manualSyncInFlight: Promise<void> | null = null;
/* OPT-022 复审：所有生产同步链共享 generation 所有权。活动表保留每个真实
   在途操作；展示优先选启动序号更新的活动，全部结束后只接受序号更新的终态。
   因而旧请求可以完成自己的业务，但不能覆盖新请求已经发布的阶段/终态。 */
interface ActiveSyncActivity {
  token: number;
  owner: SyncActivityOwner;
  stage: ManualSyncStage | null;
  startedAt: number;
  remoteWriteConfirmed: boolean;
}

interface SyncTerminal {
  token: number;
  status: 'synced' | 'error';
  failure: ManualSyncFailure | null;
  remoteWriteConfirmed: boolean;
}

const activeSyncActivities = new Map<number, ActiveSyncActivity>();
let syncActivitySequence = 0;
let latestSyncTerminal: SyncTerminal = {
  token: 0,
  status: 'synced',
  failure: null,
  remoteWriteConfirmed: false,
};

function currentSyncActivity(): ActiveSyncActivity | null {
  let current: ActiveSyncActivity | null = null;
  for (const activity of activeSyncActivities.values()) {
    if (!current || activity.token > current.token) current = activity;
  }
  return current;
}

function publishSyncActivityState(): void {
  const activity = currentSyncActivity();
  if (activity) {
    appStore().setState({
      syncStatus: 'syncing',
      syncActivityOwner: activity.owner,
      syncActivityToken: activity.token,
      syncStage: activity.stage,
      syncStartedAt: activity.startedAt,
      syncRemoteWriteConfirmed: activity.remoteWriteConfirmed,
      syncFailure: null,
    });
    return;
  }
  appStore().setState({
    syncStatus: latestSyncTerminal.status,
    syncActivityOwner: null,
    syncActivityToken: null,
    syncStage: null,
    syncStartedAt: null,
    syncRemoteWriteConfirmed: latestSyncTerminal.remoteWriteConfirmed,
    syncFailure: latestSyncTerminal.failure,
  });
}

function formatSyncWait(elapsedMs: number): string {
  const totalSec = Math.max(0, Math.floor(elapsedMs / 1000));
  if (totalSec < 60) return `${totalSec} 秒`;
  const min = Math.floor(totalSec / 60);
  const sec = totalSec % 60;
  return sec > 0 ? `${min} 分 ${sec} 秒` : `${min} 分`;
}

export const SYNC_UNRESPONSIVE_AFTER_MS = 30_000;

function syncActivityOwnerName(owner: SyncActivityOwner): string {
  switch (owner) {
    case 'manual':
      return '同步';
    case 'feed-add':
      return '添加订阅';
    case 'save-and-sync':
      return '保存并同步';
    case 'github-disconnect':
      return '断开 GitHub';
  }
}

/** OPT-022：手动同步阶段文案的唯一来源。`remoteConfirmed` 只改变“远端写回”这一
 *  事实的呈现，仍保留当前活动阶段；超过未响应阈值后明确显示“同步未返回”，
 *  但不改写 syncStatus、不释放忙锁，也不伪造成功/失败终态。 */
export function manualSyncStageLabel(
  stage: ManualSyncStage,
  opts: { remoteConfirmed?: boolean; elapsedMs?: number } = {},
): string {
  const remote = opts.remoteConfirmed === true;
  if (opts.elapsedMs != null && opts.elapsedMs >= SYNC_UNRESPONSIVE_AFTER_MS) {
    return `${remote ? '远端已写回，' : ''}同步未返回（${manualSyncStageName(stage)}，已等待 ${formatSyncWait(opts.elapsedMs)}）`;
  }
  const base =
    stage === 'feeds'
      ? '同步订阅中'
      : stage === 'states'
        ? remote
          ? '远端已写回，仍在拉取条目'
          : '同步状态并拉取条目中'
        : stage === 'refresh'
          ? remote
            ? '远端已写回，正在抓取内容'
            : '正在抓取订阅内容'
          : remote
            ? '远端已写回，正在刷新本地数据'
            : '正在刷新本地数据';
  return `${base}…`;
}

/** 失败摘要中的阶段名，避免把“内容抓取”误写成“同步完成”。 */
export function manualSyncStageName(stage: ManualSyncStage): string {
  switch (stage) {
    case 'feeds':
      return '订阅同步';
    case 'states':
      return '状态与条目同步';
    case 'refresh':
      return '内容抓取';
    case 'reload':
      return '本地刷新';
  }
}

/** 所有同步活动的展示口径：手动链带阶段；添加/保存等通用链在超时阈值前仍是
 *  “同步中”，超过阈值后明确显示具体操作未返回。 */
export function syncActivityLabel(activity: {
  owner: SyncActivityOwner;
  stage: ManualSyncStage | null;
  remoteConfirmed: boolean;
  elapsedMs: number;
}): string {
  if (activity.stage) {
    return manualSyncStageLabel(activity.stage, {
      remoteConfirmed: activity.remoteConfirmed,
      elapsedMs: activity.elapsedMs,
    });
  }
  if (activity.elapsedMs >= SYNC_UNRESPONSIVE_AFTER_MS) {
    return `${syncActivityOwnerName(activity.owner)}未返回（已等待 ${formatSyncWait(activity.elapsedMs)}）`;
  }
  return '同步中…';
}

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

/** TASK-100 P3-3：sync_phase 失败的「未连接」判定。后端未配置/未连接时
    sync_phase 抛结构化错误 { code: 'notConnected', message: '未连接后端' }
    （src-tauri/commands/sync.rs）——只有这一类允许静默跳过订阅层同步、走纯直连；
    此前 .catch(() => null) 把网络故障/超时等全部吞成「未连接」，已连接用户的
    同步失败不可见（假成功）。 */
function isNotConnectedError(e: unknown): boolean {
  if (e && typeof e === 'object') {
    const rec = e as Record<string, unknown>;
    if (rec.code === 'notConnected') return true;
  }
  return extractError(e).includes('未连接');
}

export const createSyncSlice: StateCreator<AppState, [], [], SyncSlice> = (set, get) => ({
  syncStatus: 'synced',
  syncActivityOwner: null,
  syncActivityToken: null,
  syncStage: null,
  syncStartedAt: null,
  syncRemoteWriteConfirmed: false,
  syncFailure: null,
  backgroundSyncing: false,
  syncConnected: false,
  /* TASK-116 四态展示：队列统计初值 0（未拉到统计前与「无队列」同形——X3 不制造噪音） */
  syncWaiting: 0,
  syncFailed: 0,
  /* TASK-124：最近一次推送失败摘要初值 null（无失败不制造噪音） */
  syncQueueLastError: null,
  /* OPT-009 B3：订阅写回回执/能力矩阵初值 null（未加载——不渲染、不冒充
     「无待办」；事件与挂载刷新写入同一状态源） */
  subOutboxStats: null,
  subscriptionCapabilities: null,
  githubFlow: null,
  githubAccount: null,
  githubLoggingIn: false,

  beginSyncActivity: (owner) => {
    const token = ++syncActivitySequence;
    activeSyncActivities.set(token, {
      token,
      owner,
      stage: null,
      startedAt: Date.now(),
      remoteWriteConfirmed: false,
    });
    publishSyncActivityState();
    return token;
  },

  updateSyncActivity: (token, patch: SyncActivityUpdate) => {
    const activity = activeSyncActivities.get(token);
    if (!activity) return false;
    if (patch.stage !== undefined) activity.stage = patch.stage;
    if (patch.remoteWriteConfirmed !== undefined) {
      activity.remoteWriteConfirmed = patch.remoteWriteConfirmed;
    }
    publishSyncActivityState();
    return true;
  },

  finishSyncActivity: (token, status, failure = null) => {
    const activity = activeSyncActivities.get(token);
    if (!activity) return false;
    activeSyncActivities.delete(token);
    if (token > latestSyncTerminal.token) {
      latestSyncTerminal = {
        token,
        status,
        failure: status === 'error' ? failure : null,
        remoteWriteConfirmed: activity.remoteWriteConfirmed,
      };
    }
    publishSyncActivityState();
    return true;
  },

  triggerManualSync: () => {
    if (manualSyncInFlight) return manualSyncInFlight;

    const token = get().beginSyncActivity('manual');
    const advance = (stage: ManualSyncStage, remoteConfirmed?: boolean) => {
      get().updateSyncActivity(token, {
        stage,
        remoteWriteConfirmed:
          remoteConfirmed ?? activeSyncActivities.get(token)?.remoteWriteConfirmed ?? false,
      });
    };
    const finishSynced = () => {
      get().finishSyncActivity(token, 'synced');
    };
    const finishError = (stage: ManualSyncStage, e: unknown) => {
      get().finishSyncActivity(token, 'error', { stage, message: extractError(e) });
    };

    const run = (async () => {
      if (get().dataMode !== 'tauri') {
        get().showToast('刷新中…');
        await new Promise((resolve) => setTimeout(resolve, 1100));
        finishSynced();
        get().showToast('已刷新');
        return;
      }

      let stage: ManualSyncStage = 'feeds';
      /* Note: 手动同步的阶段/远端确认/终态必须在同一推进点更新 — 见
         .agents/notes/implemented/feature/2026-10-06-同步四态与失败可见性.md */
      advance(stage);
      const syncFailures: string[] = [];
      try {
        /* 分步同步：feeds（订阅层）→ states（状态对账，只写状态不重拉列表）→
           refreshAllFeeds（内容抓取）。状态先落库、内容再抓取，最后只 reload 一次
           带出「最新内容 + 最新状态」，避免多次 reload 造成的列表闪动
           （「获取内容后再次同步状态导致闪动」的解耦）。
           TASK-058：后端在 errors 非空时仍返回 Ok（单项失败不中断整链），故必须
           **主动读取** report.errors，否则「同步有失败」在界面上表现为「成功」。 */
        let feedsReport: Awaited<ReturnType<typeof api.syncPhase>> = null;
        try {
          feedsReport = await api.syncPhase('feeds');
        } catch (e: unknown) {
          // TASK-100 P3-3：仅「未连接」类错误静默跳过（走纯直连刷新）；
          // 其余错误可见——console.warn + 汇入手动同步的失败提示，不假成功。
          if (!isNotConnectedError(e)) {
            console.warn('手动同步：订阅层同步失败', e);
            syncFailures.push(`订阅同步失败：${extractError(e)}`);
          }
        }

        if (feedsReport) {
          const fail = syncFailureMessage(feedsReport);
          if (fail) syncFailures.push(fail);
          else get().showToast('订阅同步完成，正在同步文章状态…');

          /* states 是 Rust 的复合阶段：先 push 队列，再 pull_entries，最后可能补推。
             前端不能因为 queue 事件先清空就结束本轮；必须等 IPC 返回后再推进。 */
          stage = 'states';
          advance('states');
          const statesReport = await api.syncPhase('states', true);
          const sfail = syncFailureMessage(statesReport);
          if (sfail) syncFailures.push(sfail);
          stage = 'refresh';
          advance(
            'refresh',
            (activeSyncActivities.get(token)?.remoteWriteConfirmed ?? false) || sfail === null,
          );
        } else {
          // feeds 未连接/失败时保持既有行为：跳过 states，仍执行直连内容刷新。
          stage = 'refresh';
          advance('refresh', false);
        }

        /* states 阶段已把最新状态落库（is_read/is_starred），不在此 reload——
           等下方内容抓取完成后一次性 reload，避免「状态同步→闪动→内容→再闪动」 */
        const summary: RefreshSummary | null = await api.refreshAllFeeds();

        /* TASK-111②：手动同步与后台刷新（feeds-updated）同属内容刷新——reload
           携带保位请求，落地后 Timeline 消费顶条锚回位（审计「后台刷新保留
           当前阅读位置」）。 */
        stage = 'reload';
        advance('reload');
        await get().reloadFromBackend({ keepReadingPosition: true });

        finishSynced();
        /* TASK-058：同步失败信息与「N 个源直连失败」是两类不同失败，须**共存**
           （此前只报后者，前者被吞）。

           文案口径（严格遵守 owner 边界「成功路径文案逐字不变」）：
           - **无同步失败**时，`base` 三个分支与改动前**逐字相同**（含既有
             `已刷新，新增 N 条，M 个源直连失败` 的顺序与分隔符）；
           - **有同步失败**时，把失败**前置**——本次修复的意义就是让失败被看到，
             而提示同时含「已刷新，新增 N 条」等正常信息时，若失败排在末尾，
             用户第一眼读到的是「成功」。前置不改变任何既有文案本身，只调先后。 */
        const base = summary
          ? summary.failed_feeds > 0
            ? `已刷新，新增 ${summary.new_articles} 条，${summary.failed_feeds} 个源直连失败`
            : `已刷新，新增 ${summary.new_articles} 条`
          : '已刷新，无新文章';
        get().showToast(
          syncFailures.length > 0 ? `${syncFailures.join('，')}，${base}` : base,
        );
      } catch (e: unknown) {
        const msg = extractError(e);
        finishError(stage, e);
        get().showToast(`刷新失败：${msg}`, { label: '重试', key: 'sync:manual', run: () => get().triggerManualSync() });
      }
    })();

    const tracked = run.finally(() => {
      if (manualSyncInFlight === tracked) manualSyncInFlight = null;
    });
    manualSyncInFlight = tracked;
    return tracked;
  },

  /* ================= TASK-124：sync-queue-changed 事件落地 ================= *
     Rust 在本地入队事务提交 / 即时推送与 states 推送段确认·失败 / 认证·网络
     阻塞后发 sync-queue-changed（payload = sync_queue_stats 三元组）。本动作是
     App.tsx 监听器的唯一落点——与 reloadFromBackend 的顺带刷新并存同一状态源
     （syncWaiting/syncFailed/syncQueueLastError），pill 与 SyncTab 摘要卡自动跟随。
     写队列三字段，并在手动同步的 states 复合阶段收到零队列证据时只补记
     syncRemoteWriteConfirmed；不结束 syncing。syncConnected 不在此触碰——未配置
     静默语义在 Rust 发射点（无凭据不发事件），本地模式 pill 分支不误报。 */
  applySyncQueueChanged: (stats) => {
    let changedManualEvidence = false;
    if (stats.waiting === 0 && stats.failed === 0) {
      for (const activity of activeSyncActivities.values()) {
        if (activity.owner === 'manual' && activity.stage === 'states') {
          activity.remoteWriteConfirmed = true;
          changedManualEvidence = true;
        }
      }
    }
    set({
      syncWaiting: stats.waiting,
      syncFailed: stats.failed,
      syncQueueLastError: stats.last_error ?? null,
    });
    if (changedManualEvidence) publishSyncActivityState();
  },

  /* ================= OPT-009 B3：订阅写回回执与能力矩阵 ================= *
     Rust 在订阅 outbox 入队（新增/编辑/删除/分类操作/OPML/首连推送）与执行器
     确认·失败·local_only 标记后发 subscription-outbox-changed（payload =
     当前账号 `{pending, failed, local_only, last_error}`，账号限定——异账号暂停
     项不混入）。本动作是 App.tsx 监听器的唯一落点；设置页挂载/保存链尾另经
     refreshSubscriptionOutboxStats 主动拉取一次真值。 */
  applySubscriptionOutboxChanged: (stats: SubscriptionOutboxStats) => {
    set({ subOutboxStats: stats });
  },

  refreshSubscriptionOutboxStats: async () => {
    try {
      const stats = await api.subscriptionOutboxStats();
      /* 读失败保持原值（不写零冒充「无待办」）；浏览器环境返回 null 同样跳过 */
      if (stats) set({ subOutboxStats: stats });
    } catch {
      /* 纯提示性读取失败：保持原值（Rust 侧读取失败为 Err，不返回零值） */
    }
  },

  loadSubscriptionCapabilities: async () => {
    try {
      const caps = await api.subscriptionCapabilities();
      /* 失败保持原值（不折成 null 也不折成未连接——不伪造能力声明） */
      if (caps) set({ subscriptionCapabilities: caps });
    } catch {
      /* 同上：纯提示性读取失败静默兜底 */
    }
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
    const syncToken = get().beginSyncActivity('github-disconnect');
    set({ githubLoggingIn: true });
    try {
      await api.githubLoginDisconnect();
      clearGithubPoll();
      set({ githubAccount: null, githubFlow: null });
      get().finishSyncActivity(syncToken, 'synced');
      get().showToast('已断开 GitHub 登录');
    } catch (e) {
      get().finishSyncActivity(syncToken, 'error');
      get().showToast(`断开失败：${extractError(e)}`);
    } finally {
      set({ githubLoggingIn: false });
    }
  },
});
