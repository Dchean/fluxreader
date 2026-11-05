import { useCallback, useEffect, useRef, useState } from 'react';
import { useAppStore } from '../../store';
import {
  api,
  extractError,
  extractErrorCode,
  type PendingConnection,
  type SyncQueueStats,
  type SyncStatusInfo,
} from '../../lib/api';
import { syncStateSummary, subscriptionWriteSummary, subscriptionActionLine } from '../../lib/syncPill';
import { FluxDropdown, Switch, SettingCard, ConfirmDialog } from '../primitives';
import { CacheCleanupSection } from './CacheCleanupSection';
import { ConfigSyncSection } from './ConfigSyncSection';
import { ENDPOINT_DESC, ENDPOINT_PLACEHOLDER, endpointHint } from './endpointHint';
import { syncFailureMessage } from '../../store/syncErrors';
import {
  manualSyncStageName,
  syncActivityLabel,
  SYNC_UNRESPONSIVE_AFTER_MS,
} from '../../store/slices/sync';

/* ---------- TAB 6: 同步 ---------- */

type SaveAndSyncDraft = {
  id: number;
  revision: number;
  protocol: 'greader' | 'fever';
  endpoint: string;
  username: string;
  password: string;
  connected: boolean;
  activateVersion?: number;
};

let saveAndSyncDraftSeq = 0;
let latestCommittedDraftId = 0;

function syncStatusIdentityKey(st: SyncStatusInfo): string {
  const connected = st.connected === true;
  const protocol =
    st.protocol === 'fever' || st.protocol === 'greader' ? st.protocol : '';
  return [
    connected ? '1' : '0',
    connected ? (st.account?.trim() ?? '') : '',
    connected ? protocol : '',
    connected ? (st.endpoint?.trim() ?? '') : '',
  ].join('|');
}

function nextSaveAndSyncDraftId(): number {
  saveAndSyncDraftSeq += 1;
  return saveAndSyncDraftSeq;
}

function normalizedSyncRoot(endpoint: string): string {
  const trimmed = endpoint.trim().replace(/\/+$/, '');
  try {
    const url = new URL(trimmed);
    const port = url.port || (url.protocol === 'http:' ? '80' : url.protocol === 'https:' ? '443' : '');
    return `${url.protocol}//${url.hostname.toLowerCase()}${port ? `:${port}` : ''}${url.pathname.replace(/\/+$/, '')}`;
  } catch {
    return trimmed.toLowerCase();
  }
}

function saveStageKey(draft: SaveAndSyncDraft): string {
  // Note: 表单失败按 opaque 草稿 id 分阶段复报，重试重放草稿而非当前输入 — 见 .agents/notes/implemented/feature/2026-10-16-重试入口持久可达与错误中心.md
  return `sync:save-and-sync:${draft.id}:save`;
}

function syncStageKey(draft: SaveAndSyncDraft): string {
  return `sync:save-and-sync:${draft.id}:sync`;
}

function isSupersededDraft(draft: SaveAndSyncDraft): boolean {
  return draft.id < latestCommittedDraftId;
}

function markDraftCommitted(draft: SaveAndSyncDraft): void {
  latestCommittedDraftId = Math.max(latestCommittedDraftId, draft.id);
}

export function SyncTab() {
  const showToast = useAppStore((s) => s.showToast);
  const dataMode = useAppStore((s) => s.dataMode);
  const settingsOpen = useAppStore((s) => s.settingsOpen);
  const syncStatus = useAppStore((s) => s.syncStatus);
  const backgroundSyncing = useAppStore((s) => s.backgroundSyncing);
  const syncActivityOwner = useAppStore((s) => s.syncActivityOwner);
  const syncActivityToken = useAppStore((s) => s.syncActivityToken);
  const syncStage = useAppStore((s) => s.syncStage);
  const syncStartedAt = useAppStore((s) => s.syncStartedAt);
  const syncRemoteWriteConfirmed = useAppStore((s) => s.syncRemoteWriteConfirmed);
  const syncFailure = useAppStore((s) => s.syncFailure);
  const syncConnected = useAppStore((s) => s.syncConnected);
  const syncAccount = useAppStore((s) => s.syncAccount);
  const syncProtocol = useAppStore((s) => s.syncProtocol);
  const syncEndpoint = useAppStore((s) => s.syncEndpoint);
  const syncLastSync = useAppStore((s) => s.syncLastSync);
  const syncWaiting = useAppStore((s) => s.syncWaiting);
  const syncTerminalFailed = useAppStore((s) => s.syncTerminalFailed);
  const syncLocalOnly = useAppStore((s) => s.syncLocalOnly);
  const syncBindableRetry = useAppStore((s) => s.syncBindableRetry);
  const syncRetryWait = useAppStore((s) => s.syncRetryWait);
  const syncExpiredUnconfirmed = useAppStore((s) => s.syncExpiredUnconfirmed);
  const syncQueueLastError = useAppStore((s) => s.syncQueueLastError);
  const refreshSyncStatus = useAppStore((s) => s.refreshSyncStatus);
  const refreshSyncQueueStats = useAppStore((s) => s.refreshSyncQueueStats);
  const reloadFromBackend = useAppStore((s) => s.reloadFromBackend);
  const settings = useAppStore((s) => s.settings);
  const updateSettings = useAppStore((s) => s.updateSettings);
  /** OPT-009 B3：订阅写回回执（当前账号限定）与能力矩阵——事件与挂载刷新
   *  写入同一状态源，摘要卡/能力卡直接订阅（不依赖用户手动刷新）。 */
  const subOutboxStats = useAppStore((s) => s.subOutboxStats);
  const caps = useAppStore((s) => s.subscriptionCapabilities);
  const saveAndSyncBusyOwner = useAppStore((s) => s.saveAndSyncBusyOwner);
  const beginSaveAndSync = useAppStore((s) => s.beginSaveAndSync);
  const endSaveAndSync = useAppStore((s) => s.endSaveAndSync);
  const [endpoint, setEndpoint] = useState('');
  const [protocol, setProtocol] = useState<'greader' | 'fever'>('greader');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [syncClock, setSyncClock] = useState(() => Date.now());
  const [testing, setTesting] = useState(false);
  const [disconnecting, setDisconnecting] = useState(false);
  const [retryingExpired, setRetryingExpired] = useState(false);
  const retryExpiredBusyRef = useRef(false);
  const [expiredQueueSnapshot, setExpiredQueueSnapshot] = useState<SyncQueueStats | null>(null);
  const [pendingIdentitySave, setPendingIdentitySave] = useState<SaveAndSyncDraft | null>(null);
  const saving = saveAndSyncBusyOwner !== null || disconnecting;
  const [confirmDisconnect, setConfirmDisconnect] = useState(false);
  const [showWritebackDetails, setShowWritebackDetails] = useState(false);
  const [showActionHelp, setShowActionHelp] = useState(false);
  /** 首连弹窗：本地未绑定源数（>0 弹「同步本地订阅到账户」确认） */
  const [pendingLocalSync, setPendingLocalSync] = useState(0);
  const [syncingLocal, setSyncingLocal] = useState(false);
  /** OPT-006：配置导入留下的待确认连接建议（需重新输入凭据后保存才会激活） */
  const [pendingConn, setPendingConn] = useState<PendingConnection | null>(null);
  /** 激活意图：用户点了「填入表单并激活」后记录建议版本，保存时回传后端做 CAS。
   * 普通保存（未点激活）= undefined，后端不消费任何建议。 */
  const [activateVersion, setActivateVersion] = useState<number | undefined>(undefined);
  const formRevisionRef = useRef(0);
  const formDirtyRef = useRef(false);
  const formStatusSeqRef = useRef(0);
  const formAppliedIdentityRef = useRef('');
  const settingsWasOpenRef = useRef(false);

  /* OPT-022：阶段计时只解释仍在执行的轮次；终态由 store 状态机统一写入。 */
  useEffect(() => {
    if (syncStatus !== 'syncing' || syncStartedAt == null) return;
    const timer = setInterval(() => setSyncClock(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [syncStatus, syncStartedAt]);

  const touchForm = () => {
    formRevisionRef.current += 1;
  };

  const markFormEdited = () => {
    formDirtyRef.current = true;
    touchForm();
  };

  const applyStatusToForm = useCallback((st: SyncStatusInfo) => {
    touchForm();
    setEndpoint(st.connected ? (st.endpoint ?? '') : '');
    setUsername(st.connected ? (st.account ?? '') : '');
    setPassword('');
    setProtocol(st.protocol === 'fever' ? 'fever' : 'greader');
    formDirtyRef.current = false;
    formAppliedIdentityRef.current = syncStatusIdentityKey(st);
  }, []);

  const refreshFormFromStatus = useCallback(() => {
    const seq = ++formStatusSeqRef.current;
    void refreshSyncStatus((st) => {
      /* store 写入会立刻触发 identity effect；先登记本次响应的身份，
         让该 effect 认出这不是外部连接变化，不追发第三次读取。 */
      formAppliedIdentityRef.current = syncStatusIdentityKey(st);
    }).then((st) => {
      if (!st || seq !== formStatusSeqRef.current) return;
      if (formDirtyRef.current) {
        // 响应仍是最新事实：记下身份，避免 store 更新反向触发第三次读取；
        // 但不得覆盖用户正在编辑的表单。
        formAppliedIdentityRef.current = syncStatusIdentityKey(st);
        return;
      }
      applyStatusToForm(st);
    }).catch(() => { /* 读取失败保持当前表单；不把未知折成断开 */ });
  }, [applyStatusToForm, refreshSyncStatus]);

  const ownsDraft = (draft: SaveAndSyncDraft) => formRevisionRef.current === draft.revision;

  const showSupersededDraft = () => {
    showToast('保存草稿已失效：已有更新的连接配置提交');
  };

  /* 打开设置时读取当前连接状态 */
  useEffect(() => {
    if (dataMode !== 'tauri') return;
    refreshFormFromStatus();
    /* OPT-006：待确认连接建议（配置导入只落建议，激活要走保存流程） */
    void api.syncPendingConnection().then((p) => setPendingConn(p)).catch(() => { /* 同上：静默兜底 */ });
    void refreshSyncQueueStats();
    /* OPT-009 B3：订阅写回回执与能力矩阵挂载即取一次真值（事件缺失时
       摘要卡/能力卡仍与后端一致；读取失败保持原值） */
    void useAppStore.getState().refreshSubscriptionOutboxStats();
    void useAppStore.getState().loadSubscriptionCapabilities();
  }, [dataMode, refreshFormFromStatus, refreshSyncQueueStats]);

  /* 设置弹窗隐藏但不卸载：重新打开或外部连接身份 A→B 变化时重新读一次；
     用户已编辑表单时保留输入（dirty 判定，读取返回后仍会复核）。 */
  const syncIdentityKey = [
    syncConnected ? '1' : '0',
    syncAccount ?? '',
    syncProtocol ?? '',
    syncEndpoint ?? '',
  ].join('|');
  const syncIdentityRef = useRef(syncIdentityKey);
  syncIdentityRef.current = syncIdentityKey;
  useEffect(() => {
    setExpiredQueueSnapshot(null);
    if (dataMode !== 'tauri' || !syncConnected) return;
    let active = true;
    void api.syncQueueStats().then((stats) => {
      if (active) setExpiredQueueSnapshot(stats);
    }).catch(() => { /* 未知代际不提供恢复操作 */ });
    return () => { active = false; };
  }, [dataMode, syncConnected, syncIdentityKey, syncExpiredUnconfirmed]);
  useEffect(() => {
    if (dataMode !== 'tauri') {
      settingsWasOpenRef.current = false;
      return;
    }
    const wasOpen = settingsWasOpenRef.current;
    settingsWasOpenRef.current = settingsOpen;
    if (!settingsOpen) return;
    /* 重开设置必须重新读一次（外部连接可能已变）；仅由本次响应回填造成的
       identity 变化由 applied 标记吸收，不追发第三次请求。 */
    if (!wasOpen || syncIdentityKey !== formAppliedIdentityRef.current) {
      refreshFormFromStatus();
    }
  }, [dataMode, refreshFormFromStatus, settingsOpen, syncIdentityKey]);

  /* 轻量连通测试：不落库不做同步（填表时快速验证） */
  const doTest = async () => {
    if (!endpoint.trim() || !username.trim() || !password.trim()) {
      showToast('请填写 Endpoint、用户名和密码');
      return;
    }
    setTesting(true);
    try {
      const msg = await api.syncTest(protocol, endpoint.trim(), username.trim(), password.trim());
      showToast(msg ?? '连接成功');
    } catch (e) {
      showToast(`连接失败：${endpointHint(extractError(e))}`);
    } finally {
      setTesting(false);
    }
  };

  /** 保存后的同步阶段：只消费已成功保存的草稿，绝不重跑 sync_save。 */
  async function runPostSaveSync(draft: SaveAndSyncDraft, manageSaving = true): Promise<void> {
    if (isSupersededDraft(draft)) {
      showSupersededDraft();
      return;
    }
    if (manageSaving && !beginSaveAndSync(draft.id)) {
      showToast('已有同步操作进行中，请稍候', {
        label: '重试',
        key: syncStageKey(draft),
        run: () => { void runPostSaveSync(draft); },
      });
      return;
    }
    const syncToken = useAppStore.getState().beginSyncActivity('save-and-sync');
    try {
      const failures: string[] = [];
      let failureStage: 'feeds' | 'states' = 'states';
      let removedFeeds = 0;
      let refreshError: string | null = null;
      let stage: 'feeds' | 'states' | 'refresh' | 'reload' = 'feeds';
      useAppStore.getState().updateSyncActivity(syncToken, { stage });
      try {
        const result = await api.syncRun();
        if (!result) throw new Error('同步服务未返回本轮结果');
        if (result.error) {
          stage = result.error.stage;
          throw result.error;
        }
        if (!result.feeds || !result.states) throw new Error('同步服务未返回完整阶段结果');
        for (const current of ['feeds', 'states'] as const) {
          const report = result[current];
          const failure = syncFailureMessage(report);
          if (failure) {
            if (failures.length === 0) failureStage = current;
            failures.push(failure);
          }
          removedFeeds += report?.removed_feeds ?? 0;
        }
        stage = 'reload';
        useAppStore.getState().updateSyncActivity(syncToken, { stage });
        await reloadFromBackend();
        stage = 'refresh';
        useAppStore.getState().updateSyncActivity(syncToken, { stage });
        try {
          await api.refreshAllFeeds();
        } catch (e) {
          refreshError = extractError(e).trim() || '内容抓取失败（原因未提供）';
        }
        stage = 'reload';
        useAppStore.getState().updateSyncActivity(syncToken, { stage });
        await reloadFromBackend();
        if (refreshError !== null) {
          useAppStore.getState().finishSyncActivity(syncToken, 'error', {
            stage: 'refresh',
            message: refreshError,
          });
          showToast(`后台同步失败：${refreshError}`, {
            label: '重试',
            key: syncStageKey(draft),
            run: () => { void runPostSaveSync(draft); },
          });
          return;
        }
      } catch (e) {
        useAppStore.getState().finishSyncActivity(syncToken, 'error', {
          stage,
          message: extractError(e),
        });
        showToast(`后台同步失败：${extractError(e)}`, {
          label: '重试',
          key: syncStageKey(draft),
          run: () => { void runPostSaveSync(draft); },
        });
        return;
      }
      if (failures.length > 0) {
        useAppStore.getState().finishSyncActivity(syncToken, 'error', {
          stage: failureStage,
          message: failures.join('；'),
        });
        showToast(failures.join('；'), {
          label: '重试',
          key: syncStageKey(draft),
          run: () => { void runPostSaveSync(draft); },
        });
      } else {
        useAppStore.getState().finishSyncActivity(syncToken, 'synced');
        showToast('账户同步完成');
      }
      if (removedFeeds > 0) showToast(`本次对账移除 ${removedFeeds} 个已在同步账户删除的订阅源`);
    } finally {
      void refreshSyncStatus();
      void refreshSyncQueueStats();
      void useAppStore.getState().refreshSubscriptionOutboxStats();
      void useAppStore.getState().loadSubscriptionCapabilities();
      if (manageSaving) endSaveAndSync(draft.id);
    }
  }

  /** 保存并同步：保存（轻量测试+事务落库）→ 后台同步阶段。
      草稿一旦捕获便只使用该快照；save 失败与 post-save sync 失败各留可重试入口。
      表单 UI 写入仅在当前 revision 仍属于同一草稿时允许。 */
  async function runSaveAndSync(draft: SaveAndSyncDraft): Promise<void> {
    if (isSupersededDraft(draft)) {
      showSupersededDraft();
      return;
    }
    if (!draft.endpoint) {
      showToast('请填写 Endpoint');
      return;
    }
    if ((!draft.username || !draft.password) && !draft.connected) {
      showToast('请填写用户名和密码');
      return;
    }
    if (!beginSaveAndSync(draft.id)) {
      showToast('已有同步操作进行中，请稍候', {
        label: '重试',
        key: saveStageKey(draft),
        run: () => { void runSaveAndSync(draft); },
      });
      return;
    }
    try {
      let result: Awaited<ReturnType<typeof api.syncSave>>;
      try {
        result = await api.syncSave(
          draft.protocol,
          draft.endpoint,
          draft.username,
          draft.password,
          draft.activateVersion,
        );
      } catch (e) {
        showToast(`保存失败：${endpointHint(extractError(e))}`, {
          label: '重试',
          key: saveStageKey(draft),
          run: () => { void runSaveAndSync(draft); },
        });
        const errorCode = extractErrorCode(e);
        if (ownsDraft(draft)) {
          void api.syncPendingConnection().then((p) => {
            if (!ownsDraft(draft)) return;
            setPendingConn(p);
            if (errorCode === 'staleActivation') setActivateVersion(undefined);
          }).catch(() => { /* 静默兜底 */ });
        }
        return;
      }

      markDraftCommitted(draft);
      const stillOwned = ownsDraft(draft);
      if (stillOwned) {
        formDirtyRef.current = false;
        setPassword('');
        if (result?.firstConnect && result.unboundLocalFeeds > 0) {
          setPendingLocalSync(result.unboundLocalFeeds);
        }
      }
      setActivateVersion((current) => (
        current === draft.activateVersion ? undefined : current
      ));
      void api.syncPendingConnection().then((p) => {
        if (ownsDraft(draft)) setPendingConn(p);
      }).catch(() => { /* 静默兜底 */ });
      refreshFormFromStatus();
      showToast(result?.message ?? '已保存，正在后台同步…');
      await runPostSaveSync(draft, false);
    } finally {
      endSaveAndSync(draft.id);
    }
  }

  const doSaveAndSync = () => {
    const draft: SaveAndSyncDraft = {
      id: nextSaveAndSyncDraftId(),
      revision: formRevisionRef.current,
      protocol,
      endpoint: endpoint.trim(),
      username: username.trim(),
      password: password.trim(),
      connected: syncConnected,
      activateVersion,
    };
    const identityChanged = syncConnected && (
      draft.protocol !== (syncProtocol ?? 'greader')
      || normalizedSyncRoot(draft.endpoint) !== normalizedSyncRoot(syncEndpoint ?? '')
      || (draft.username || syncAccount) !== syncAccount
    );
    if (identityChanged) setPendingIdentitySave(draft);
    else void runSaveAndSync(draft);
  };

  const doRetryExpired = async () => {
    const generation = expiredQueueSnapshot?.account_generation;
    if (retryExpiredBusyRef.current || generation == null || !syncConnected) return;
    const identity = syncIdentityKey;
    retryExpiredBusyRef.current = true;
    setRetryingExpired(true);
    try {
      const result = await api.syncRetryExpired(generation);
      if (identity !== syncIdentityRef.current) return;
      if (!result) throw new Error('同步服务未返回重试结果');
      showToast(`已重新排队 ${result.requeued} 项：待推送 ${result.bound_pending}，等待身份 ${result.bindable_retry}，仅本机 ${result.local_only}${result.remaining_expired ? `；仍有 ${result.remaining_expired} 项过期未确认` : ''}。待推送项将在下次同步时重试`);
    } catch (error) {
      if (identity === syncIdentityRef.current) showToast(`重试过期项失败：${extractError(error)}`);
    } finally {
      retryExpiredBusyRef.current = false;
      setRetryingExpired(false);
      void refreshSyncQueueStats();
      if (identity === syncIdentityRef.current) {
        void api.syncQueueStats().then((stats) => {
          if (identity === syncIdentityRef.current) setExpiredQueueSnapshot(stats);
        }).catch(() => {
          if (identity === syncIdentityRef.current) setExpiredQueueSnapshot(null);
        });
      }
    }
  };
  /** OPT-006：把配置导入的待确认建议填入表单（不激活——必须重新输入密码后保存） */
  const doApplyPending = () => {
    if (!pendingConn) return;
    markFormEdited();
    if (pendingConn.sync_protocol === 'fever' || pendingConn.sync_protocol === 'greader') {
      setProtocol(pendingConn.sync_protocol);
    }
    if (pendingConn.greader_endpoint) setEndpoint(pendingConn.greader_endpoint);
    if (pendingConn.greader_username) setUsername(pendingConn.greader_username);
    setPassword('');
    /* 记录激活版本：保存时回传后端（后端独立做版本/身份 CAS，不信任 UI） */
    setActivateVersion(pendingConn.version);
    showToast('已填入待确认连接信息：请重新输入密码后点「保存并同步」激活');
  };

  /** OPT-006：放弃待确认建议（不触碰当前连接与本地数据） */
  const doDismissPending = async () => {
    try {
      await api.syncDismissPendingConnection();
      touchForm();
      setPendingConn(null);
      setActivateVersion(undefined);
      showToast('已忽略配置导入的连接建议');
    } catch (e) {
      showToast(`忽略失败：${extractError(e)}`);
    }
  };

  /** 把本地直连订阅推送到后端（首连弹窗确认与手动按钮共用）。
   * 幂等：已绑定的跳过、服务端已有同 URL（409）回查绑定不报错 */
  const doSyncLocal = async () => {
    if (dataMode !== 'tauri') {
      showToast('演示模式不支持同步');
      return;
    }
    setSyncingLocal(true);
    try {
      const result = await api.syncLocalFeeds();
      await reloadFromBackend();
      setPendingLocalSync(0);
      void refreshSyncStatus();
      void refreshSyncQueueStats();
      void useAppStore.getState().refreshSubscriptionOutboxStats();
      if (!result) {
        showToast('同步完成');
      } else if (result.ok) {
        showToast(result.message || '同步完成');
      } else {
        const detail = result.errors.join('；') || result.message || '同步服务返回失败';
        showToast(
          result.structured === false
            ? `同步本地订阅，结果未知：${detail}`
            : result.failed > 0
              ? `同步本地订阅，${result.failed} 项未同步：${detail}`
              : detail,
          { label: '重试', key: 'sync:local-feeds', run: () => { void doSyncLocal(); } },
        );
      }
    } catch (e) {
      showToast(`同步本地订阅失败：${extractError(e)}`);
    } finally {
      setSyncingLocal(false);
    }
  };

  const doDisconnect = async () => {
    /* 断开是连接身份切换：先推进表单请求代次，任何在途旧 status 响应都不得
       再把已断开的表单写回旧账户。 */
    formStatusSeqRef.current += 1;
    setDisconnecting(true);
    try {
      const msg = await api.syncDisconnect();
      useAppStore.getState().applySyncStatus(null);
      useAppStore.setState({ syncSucceededAt: null });
      applyStatusToForm({
        connected: false,
        endpoint: null,
        account: null,
        last_sync: 0,
        protocol: null,
      });
      /* OPT-009 B3：断开后协议/账号不再生效——订阅回执与能力矩阵重读 */
      void useAppStore.getState().refreshSubscriptionOutboxStats();
      void useAppStore.getState().loadSubscriptionCapabilities();
      /* TASK-100 P3-8：断开与「断开后的本地刷新」分开处理——此前 reload 失败
         重抛被同一个 catch 捕获，把「已断开成功」误报成「断开失败」。 */
      try {
        await reloadFromBackend();
      } catch (reloadErr) {
        showToast(`已断开连接，但本地刷新失败：${extractError(reloadErr)}`);
        return;
      }
      showToast(msg ?? '已断开连接');
    } catch (e) {
      showToast(`断开失败：${extractError(e)}`);
    } finally {
      void refreshSyncStatus();
      setDisconnecting(false);
    }
  };

  if (dataMode !== 'tauri') {
    return (
      <>
        <div className="settings-group-title">同步服务</div>
        <SettingCard title="演示模式" desc="同步功能需要运行在 Tauri 客户端内（npm run tauri dev）">
          <span className="about-arch-tag">不支持同步</span>
        </SettingCard>
      </>
    );
  }

  /* TASK-102 X3：账户前缀提为常量——空串字面量与「从未」引号留在同一行时，
     文案扫描会把两个引号间的模板内容误配成假长文案（'…' 对跨模板配对）；
     分行后字面量配对只在单行内成立，扫描恢复零误报。渲染输出不变。 */
  const accountTag = syncAccount ? `账户 ${syncAccount} · ` : '';

  /* TASK-116 X2：四态摘要口径收口到纯函数 syncStateSummary（src/lib/syncPill.ts，
     契约 X2/X3 如实原则；真值表由回归网 t116 断言驱动）。
     OPT-009 B3：订阅写回回执行由 subscriptionWriteSummary 纯函数追加——无待办
     时不新增噪音行；local_only 项如实点名「仅本地生效」（不写成成功也不隐藏）。 */
  const waiting = syncWaiting;
  const failedCount =
    syncTerminalFailed + syncRetryWait + (subOutboxStats?.failed ?? 0);
  const unresolvedCount =
    syncLocalOnly
    + syncBindableRetry
    + syncExpiredUnconfirmed
    + (subOutboxStats?.pending ?? 0)
    + (subOutboxStats?.local_only ?? 0);
  const syncElapsedMs = syncStartedAt == null
    ? 0
    : Math.max(0, syncClock - syncStartedAt);
  const syncProgressLine =
    syncStatus === 'syncing' && syncActivityOwner && syncActivityToken != null
      ? `整轮同步进行中：${syncActivityLabel({
          owner: syncActivityOwner,
          stage: syncStage,
          remoteConfirmed: syncRemoteWriteConfirmed,
          elapsedMs: syncElapsedMs,
        })}`
      : '';
  const syncFailureLine = syncFailure
    ? `上次同步失败于${manualSyncStageName(syncFailure.stage)}：${syncFailure.message}`
    : '';
  const subscriptionUnknown = subOutboxStats == null;
  const subscriptionLine = subscriptionUnknown
    ? '订阅状态未加载（未知）'
    : subscriptionWriteSummary(subOutboxStats, caps?.protocol ?? null);
  const syncStateDesc = [
    syncProgressLine,
    syncFailureLine,
    syncStateSummary({
      waiting: syncWaiting,
      failed: syncTerminalFailed,
      retry_wait: syncRetryWait,
      local_only: syncLocalOnly,
      bindable_retry: syncBindableRetry,
      expired_unconfirmed: syncExpiredUnconfirmed,
      last_error: syncQueueLastError,
    }, syncLastSync),
    subscriptionLine,
  ].filter(Boolean).join('；');
  const syncStateTag =
    syncStatus === 'syncing' || backgroundSyncing
      ? syncElapsedMs >= SYNC_UNRESPONSIVE_AFTER_MS
        ? '未返回'
        : '同步中'
      : syncStatus === 'error'
        ? '同步失败'
        : failedCount > 0
          ? '失败'
          : waiting > 0
            ? '状态待推送'
            : unresolvedCount > 0
              ? '未同步'
              : subscriptionUnknown
                ? '订阅未知'
                : '本地已保存';

  return (
    <>
      <div className="settings-group-title">同步服务</div>
      <SettingCard
        title="连接状态"
        desc={syncConnected
          ? syncAccount
            ? `${accountTag}上次同步 ${syncLastSync > 0 ? new Date(syncLastSync * 1000).toLocaleString() : '从未'}`
            : '未验证：已配置同步服务地址，但缺少账户信息'
          : '未连接（客户端可独立使用：直连抓取、阅读、收藏均正常）'}
      >
        <span className="about-arch-tag">
          {syncConnected ? (syncAccount ?? '未验证（账户缺失）') : '未连接'}
        </span>
      </SettingCard>
      {/* OPT-006：配置导入的待确认连接建议——只展示非敏感字段；激活必须重新输入
          密码后经「保存并同步」专用流程（历史实现的导入直接改写活动地址 + 旧密码，
          会把旧密码拼给新服务端）。 */}
      {pendingConn && (
        <SettingCard
          title="配置导入的连接建议"
          desc={`来自配置同步：${pendingConn.sync_protocol === 'fever' ? 'Fever' : 'GReader'} · ${pendingConn.greader_endpoint ?? '（未提供地址）'} · ${pendingConn.greader_username ?? '（未提供用户名）'}。激活需重新输入密码后保存，当前连接不受影响。`}
        >
          <div className="settings-action-row">
            <button className="toggle-action-btn btn-primary" disabled={saving} onClick={doApplyPending}>
              填入表单并激活
            </button>
            <button className="toggle-action-btn" disabled={saving} onClick={() => void doDismissPending()}>
              忽略
            </button>
          </div>
        </SettingCard>
      )}
      {/* TASK-116 X2：四态摘要卡。状态标签 = 四态中的当下态（部分失败 > 等待同步 >
         本地已保存）——「远端已确认」随成功 prune 即时出队、不可作常驻态展示（X3）；
         desc 收尾即「本地已保存」说明句（状态变更已事务化落库，TASK-108） */}
      <SettingCard title="同步状态" desc={syncStateDesc}>
        <div className="settings-action-row">
          <span className="about-arch-tag">{syncStateTag}</span>
          {syncExpiredUnconfirmed > 0 && syncConnected && (
            <button
              className="toggle-action-btn"
              disabled={saving || retryingExpired || expiredQueueSnapshot?.account_generation == null}
              title="按当前本地已读与收藏状态重新排队；没有远端身份的项仍需等待绑定"
              onClick={() => void doRetryExpired()}
            >
              {retryingExpired ? '正在重排…' : '重试过期项'}
            </button>
          )}
        </div>
      </SettingCard>
      {/* OPT-009 B3：订阅写回能力卡——概述常驻，矩阵/限制来自同一份
          subscription_capabilities 数据，按需展开。未连接不渲染。
          Note: 主界面单句、细节折叠 — 见 .agents/notes/implemented/simplification/2026-11-04-用户文案精简与能力矩阵折叠.md */}
      {caps?.connected && (
        <>
          <SettingCard title="订阅写回" desc={caps.overview}>
            <span className="about-arch-tag">{caps.protocol === 'fever' ? '仅本地' : '写回账户'}</span>
          </SettingCard>
          <div className="subscription-details-toggle">
            <button
              className="toggle-action-btn"
              aria-expanded={showWritebackDetails}
              aria-controls="subscription-writeback-details"
              onClick={() => setShowWritebackDetails((open) => !open)}
            >
              {showWritebackDetails ? '收起写回细节' : '查看写回细节'}
            </button>
          </div>
          {showWritebackDetails && (
            <div
              id="subscription-writeback-details"
              className="subscription-details-body"
            >
              <div>{subscriptionActionLine(caps)}</div>
              {caps.limitations.map((l) => (
                <div key={l.code}>· {l.detail}</div>
              ))}
            </div>
          )}
        </>
      )}
      {/* REQ-008：全应用统一控件——此前是全仓唯一的原生 <select>，
          深浅主题外观与展开行为都与 FluxDropdown 不一致。
          TASK-102：FluxDropdown 必须是 SettingCard 的直接子元素（.setting-card
          双列布局的控件列贴右缘）——包进裸 div 会被提示文本撑宽、把下拉挤离
          右缘（X1 错位根因）；API 密码提示改放 desc 位（≤32 字），不设常驻 hint。 */}
      <SettingCard
        title="同步协议"
        desc="Fever / GReader 均用「API 密码」，非登录密码"
      >
        <FluxDropdown
          width={220}
          value={protocol}
          onChange={(v) => {
            markFormEdited();
            setProtocol(v === 'fever' ? 'fever' : 'greader');
          }}
          options={[
            { value: 'greader', label: 'Google Reader（推荐）' },
            { value: 'fever', label: 'Fever' },
          ]}
        />
      </SettingCard>
      <SettingCard
        title="同步服务地址"
        desc={ENDPOINT_DESC}
      >
        <input
          type="text"
          aria-label="同步服务地址"
          className="setting-input"
          placeholder={ENDPOINT_PLACEHOLDER}
          value={endpoint}
          onChange={(e) => {
            markFormEdited();
            setEndpoint(e.target.value);
          }}
        />
      </SettingCard>
      <SettingCard
        title="用户名"
        desc="同步服务生成的 API 用户名；Miniflux 在「集成」页查看（非登录密码）"
      >
        <input
          type="text"
          aria-label="用户名"
          className="setting-input"
          placeholder="集成用户名"
          value={username}
          onChange={(e) => {
            markFormEdited();
            setUsername(e.target.value);
          }}
        />
      </SettingCard>
      {/* TASK-102：断开态不设 desc——「集成密码」字面解释复述标题，共用语义已由
          协议卡 desc 与用户名卡承载（X3：无复述标题的 desc） */}
      <SettingCard
        title="密码"
        desc={syncConnected ? '已保存且不回显。留空保持密码；填写新值只更新密码；换账号需同时改地址或用户名。' : undefined}
      >
        <input
          type="password"
          aria-label="密码"
          className="setting-input"
          placeholder={syncConnected ? '●●●●●●●●（已保存）' : '密码'}
          value={password}
          onChange={(e) => {
            markFormEdited();
            setPassword(e.target.value);
          }}
        />
      </SettingCard>
      <div
        className="settings-action-block"
        onFocus={() => setShowActionHelp(true)}
        onBlur={(event) => {
          if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
            setShowActionHelp(false);
          }
        }}
        onMouseEnter={() => setShowActionHelp(true)}
        onMouseLeave={() => setShowActionHelp(false)}
      >
        <div className="settings-action-row">
          <button
            className="toggle-action-btn"
            disabled={testing || saving}
            onClick={() => void doTest()}
            title="只验证登录与地址，不会保存配置，也不拉取订阅或文章"
            aria-describedby="sync-test-help"
          >
            {testing ? '测试中…' : '测试连接'}
          </button>
          <button
            className="toggle-action-btn btn-primary"
            disabled={testing || saving}
            onClick={() => void doSaveAndSync()}
            title="保存后拉取订阅、阅读状态与文章并刷新内容；不会推送已有本地订阅"
            aria-describedby="sync-save-help"
          >
            {saving ? '保存中…' : '保存并同步'}
          </button>
          {syncConnected && syncProtocol !== 'fever' && (
            <button
              className="toggle-action-btn"
              disabled={syncingLocal || saving}
              onClick={() => void doSyncLocal()}
              title="把本地直连添加的订阅推送到同步账户（已绑定的自动跳过）"
            >
              {syncingLocal ? '同步中…' : '同步本地订阅'}
            </button>
          )}
          {syncConnected && (
            <button className="toggle-action-btn" disabled={testing || saving} onClick={() => setConfirmDisconnect(true)}>
              断开连接
            </button>
          )}
        </div>
        <div
          id="sync-action-help"
          className="settings-action-help"
          hidden={!showActionHelp}
          role="note"
        >
          <p id="sync-test-help">测试连接：只验证登录与地址，不会保存配置，也不拉取订阅或文章。</p>
          <p id="sync-save-help">保存并同步：保存后拉取订阅、阅读状态与文章并刷新内容；不会推送已有本地订阅。</p>
        </div>
      </div>

      <div className="settings-group-title" style={{ marginTop: 20 }}>自动同步</div>
      <SettingCard
        title="同步模式"
        desc="本机抓取：离线可用；跟随同步服务：多设备一致"
      >
        <FluxDropdown
          width={130}
          value={settings.syncMode}
          onChange={(v) => updateSettings({ syncMode: v as 'direct' | 'hybrid' })}
          options={[
            { value: 'direct', label: '本机抓取' },
            { value: 'hybrid', label: '跟随同步服务' },
          ]}
        />
      </SettingCard>
      <SettingCard title="后台自动同步" desc="按刷新间隔增量同步账户状态">
        <Switch checked={settings.autoSync} onChange={(v) => updateSettings({ autoSync: v })} />
      </SettingCard>

      <CacheCleanupSection />
      <ConfigSyncSection />

      <ConfirmDialog
        open={confirmDisconnect}
        title="断开同步服务"
        message="断开后将移除从同步服务拉取的订阅与文章，并放弃尚未确认的已读/收藏同步队列。本地直连订阅及其已读/收藏值保留，但这些旧意图不会在重连后自动补推。确定断开吗？"
        confirmText="断开并清理"
        onConfirm={() => { setConfirmDisconnect(false); void doDisconnect(); }}
        onCancel={() => setConfirmDisconnect(false)}
      />

      <ConfirmDialog
        open={pendingIdentitySave !== null}
        title="更换同步账户"
        message="更换账户将清理原账户的远端订阅与文章，并放弃尚未确认的已读/收藏同步队列。本地直连订阅及其已读/收藏值保留，旧意图不会迁移或自动补推到新账户。确定更换吗？"
        confirmText="更换并清理"
        onConfirm={() => {
          const draft = pendingIdentitySave;
          setPendingIdentitySave(null);
          if (draft) void runSaveAndSync(draft);
        }}
        onCancel={() => setPendingIdentitySave(null)}
      />

      {/* 首连且本地有未绑定源：询问是否把本地订阅推送到服务端
          （推送会改变服务端数据——必须用户确认，不自动执行） */}
      <ConfirmDialog
        open={pendingLocalSync > 0}
        title="同步本地订阅到账户"
        message={`检测到本地有 ${pendingLocalSync} 个直连添加的订阅尚未同步到账户。是否现在同步？同步后它们会出现在你的同步账户中，其他设备也能看到。`}
        confirmText="同步到账户"
        onConfirm={() => { setPendingLocalSync(0); void doSyncLocal(); }}
        onCancel={() => setPendingLocalSync(0)}
      />
    </>
  );
}
