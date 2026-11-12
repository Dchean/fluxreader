import { suspendImages, resumeImages } from '../../lib/imageProxy';
import { awaitCoverReportFrontier } from '../../lib/coverImage';
import type { StateCreator } from 'zustand';
import { api, awaitLocalFlagWriteFrontier, extractError, extractErrorCode, runPlaybackRestoreBoundary } from '../../lib/api';
import type { LocalSyncStatus } from '../../lib/api';
import { enqueueAiConfigWrite, invalidateAiConfigDraftsAfterRestore } from '../../components/settings/aiConfig';
import { awaitAiWriteFrontier, markAiConfigUnconfirmed, runConfigApplyBarrier } from '../aiConfigWrites';
import { confirmAppliedConfig, resetLibraryViewAfterRestore } from './bootstrap';
import { awaitAppSettingsWriteFrontier, markAppSettingsUnconfirmed, readBackAppliedAppSettings, resetAppSettingsForLibraryRestore, runAppSettingsApplyBoundary } from './settings';
import type { AppState } from '../types';

type LocalLibrarySlice = Pick<AppState,
  | 'libraryGeneration' | 'libraryRestoreBusy' | 'localSyncState' | 'localSyncError' | 'localSyncBusy'
  | 'refreshLocalSyncStatus' | 'connectLocalSync' | 'disconnectLocalSync' | 'runLocalSync'
  | 'portableBackup' | 'choosePortableBackup' | 'cancelPortablePreview' | 'restorePortableBackup'
  | 'retryRestoredLibraryRefresh'>;

/** 每个读取/连接操作有自己的响应序号，旧状态回包不能回滚断开或新连接。 */
let statusSerial = 0;

export const createLocalLibrarySlice: StateCreator<AppState, [], [], LocalLibrarySlice> = (set, get) => {
  const patchBackup = (patch: Partial<AppState['portableBackup']>) => set((s) => ({ portableBackup: { ...s.portableBackup, ...patch } }));
  const landStatus = (status: LocalSyncStatus) => {
    if (!get().localSyncState || status.generation >= get().localSyncState!.generation) set({ localSyncState: status });
  };
  const runConnection = async (kind: 'connect' | 'disconnect' | 'run', op: () => Promise<LocalSyncStatus>): Promise<boolean> => {
    if (get().localSyncBusy || get().libraryRestoreBusy) return false;
    if (kind === 'connect' && (get().syncConnected || get().saveAndSyncBusyOwner !== null)) {
      set({ localSyncError: '请先断开 RSS 同步服务，再连接 WebDAV' }); return false;
    }
    set({ localSyncBusy: kind, localSyncError: null });
    ++statusSerial; // 废弃操作前已发出的状态读取。
    try {
      const status = await op();
      ++statusSerial; // 操作期间的读取也不能覆盖命令返回的新状态。
      landStatus(status);
      // state返回与独立body事件可以交错；命令快照落地后再取一次真实终态。
      try { await get().refreshLocalSyncStatus(); } catch { /* 状态读取错误由近端提示保留 */ }
      get().showToast(kind === 'connect' ? 'WebDAV 连接已保存；同步结果请查看下方状态' : kind === 'disconnect' ? '已断开 WebDAV，本地资料保留' : '本轮状态同步已完成；正文补齐单独进行');
      return true;
    } catch (error) {
      const message = `${kind === 'connect' ? '连接失败' : kind === 'disconnect' ? '断开失败' : '本轮状态同步未完成'}：${extractError(error)}`;
      set({ localSyncError: message });
      get().showToast(message);
      // 失败也回读 pending/下一重试时间；读取失败不擦除实际操作错误。
      try { await get().refreshLocalSyncStatus(); } catch { /* 两个错误均保留在同一近端提示 */ }
      set({ localSyncError: message });
      return false;
    } finally { set({ localSyncBusy: null }); }
  };
  const refreshRestoredLibrary = async (): Promise<void> => {
    const failures: string[] = [];
    const attempt = async (label: string, action: () => Promise<unknown>) => {
      try { await action(); } catch (error) { failures.push(`${label}：${extractError(error)}`); }
    };
    await attempt('设置回读失败', async () => {
      try { await readBackAppliedAppSettings(); } catch (error) { markAppSettingsUnconfirmed(); throw error; }
    });
    await attempt('AI 开关回读失败', async () => {
      try { await confirmAppliedConfig(); } catch (error) { markAiConfigUnconfirmed(); throw error; }
    });
    await attempt('AI 服务回读失败', async () => {
      const raw = await api.getAiConfig();
      if (raw) JSON.parse(raw);
    });
    await attempt('RSS 状态回读失败', async () => {
      const state = await get().refreshSyncStatus();
      if (!state) throw new Error('后端未返回状态');
      const [queue, outbox, capabilities] = await Promise.all([api.syncQueueStats(), api.subscriptionOutboxStats(), api.subscriptionCapabilities()]);
      if (!queue || !outbox || !capabilities) throw new Error('同步状态回读不完整');
      get().applySyncQueueChanged(queue);
      get().applySubscriptionOutboxChanged(outbox);
      set({ subscriptionCapabilities: capabilities });
    });
    await attempt('WebDAV 状态回读失败', () => get().refreshLocalSyncStatus());
    await attempt('资料库界面刷新失败', () => get().reloadFromBackend());
    if (failures.length) throw new Error(failures.join('；'));
  };
  const reportRefreshFailure = (error: unknown) => {
    const message = `资料库恢复已完成，但界面刷新失败：${extractError(error)}`;
    patchBackup({ error: message, refreshFailed: true });
    get().showToast(message, { label: '重试界面刷新', key: 'portable:refresh', run: () => { void get().retryRestoredLibraryRefresh(); } });
  };
  return {
    libraryGeneration: 0, libraryRestoreBusy: false,
    localSyncState: null, localSyncError: null, localSyncBusy: null,
    portableBackup: { busy: null, preview: null, result: null, error: null, refreshFailed: false },
    refreshLocalSyncStatus: async () => {
      const serial = ++statusSerial;
      try {
        const status = await api.localSyncStatus();
        if (!status) throw new Error('WebDAV 状态仅在 Windows 客户端可用');
        if (serial === statusSerial) { landStatus(status); set({ localSyncError: null }); }
        return status;
      } catch (error) {
        if (serial === statusSerial) set({ localSyncError: `状态读取失败：${extractError(error)}` });
        throw error;
      }
    },
    connectLocalSync: (config) => runConnection('connect', () => api.localSyncConnect(config)),
    disconnectLocalSync: async () => { await runConnection('disconnect', () => api.localSyncDisconnect()); },
    runLocalSync: async () => { await runConnection('run', () => api.localSyncRun()); },
    choosePortableBackup: async (save) => {
      if (get().portableBackup.busy || get().portableBackup.refreshFailed || get().libraryRestoreBusy) return;
      patchBackup({ busy: save ? 'export' : 'preview', preview: null, error: null });
      try {
        const path = await api.choosePortableBackupFile(save);
        if (!path) return; // native picker cancellation never invokes export/restore.
        if (save) {
          await Promise.all([awaitAiWriteFrontier(), awaitAppSettingsWriteFrontier(), awaitLocalFlagWriteFrontier(), enqueueAiConfigWrite(async () => {})]);
          const info = await api.exportPortableBackup(path);
          patchBackup({ result: { kind: 'export', info }, refreshFailed: false });
          get().showToast(`便携备份已导出：收藏 ${info.starred} 篇，缺正文 ${info.missing_bodies} 篇`);
        } else {
          const info = await api.previewPortableBackup(path);
          patchBackup({ preview: { path, info } });
        }
      } catch (error) {
        const message = `${save ? '导出失败' : '预览失败'}：${extractError(error)}`;
        patchBackup({ error: message }); get().showToast(message);
      } finally { patchBackup({ busy: null }); }
    },
    cancelPortablePreview: () => { if (!get().portableBackup.busy) patchBackup({ preview: null, error: null }); },
    restorePortableBackup: async () => {
      const preview = get().portableBackup.preview;
      if (!preview || get().portableBackup.busy || get().libraryRestoreBusy) return;
      if (get().localSyncBusy || get().saveAndSyncBusyOwner !== null || get().syncStatus === 'syncing') {
        patchBackup({ error: '同步操作尚未结束，请稍候再确认恢复' }); return;
      }
      suspendImages();
      // 同步置忙，由 App inert/键盘守卫阻止新用户写；保留已有写前沿，不清空队列。
      set((s) => ({ libraryRestoreBusy: true, playerExpanded: false,
        player: { ...s.player, isActive: false, isPlaying: false, audioUrl: '', title: '', showName: '', cover: '', coverEntryId: '', coverPageUrl: undefined, positionSec: 0, durationSec: 0, seekToSec: null } }));
      // Note: 先使画廊旧请求失效，不能让无首图详情在读藏前沿之后补发标读；失败也不自动重开 — 见 .agents/notes/implemented/architecture/2026-11-09-本地同步与便携恢复界面边界.md
      get().closeLightbox();
      patchBackup({ busy: 'restore', preview: null, result: null, error: null, refreshFailed: false });
      try {
        await runPlaybackRestoreBoundary(() => runConfigApplyBarrier(() => runAppSettingsApplyBoundary(() => enqueueAiConfigWrite(async () => {
          await awaitLocalFlagWriteFrontier();
          await awaitCoverReportFrontier();
          const info = await api.restorePortableBackup(preview.path, preview.info.digest);
          // 从这里起任何异常都是“已恢复，刷新未完成”，不能重放 restore。
          patchBackup({ result: { kind: 'restore', info }, busy: 'refresh' });
          invalidateAiConfigDraftsAfterRestore();
          resetLibraryViewAfterRestore();
          resetAppSettingsForLibraryRestore();
          ++statusSerial;
          try { await refreshRestoredLibrary(); }
          catch (error) { reportRefreshFailure(error); }
        }))));
        if (!get().portableBackup.refreshFailed) get().showToast('资料库恢复已完成；同步与 AI 服务需要重新输入凭据');
      } catch (error) {
        if (get().portableBackup.result?.kind === 'restore') reportRefreshFailure(error);
        else {
          const suffix = extractErrorCode(error) === 'staleBackupPreview' ? '；文件已变化，请重新选择并预览' : '；当前资料库保留，请重新预览后再试';
          const message = `恢复失败：${extractError(error)}${suffix}`;
          patchBackup({ error: message }); get().showToast(message);
        }
      } finally { resumeImages(); patchBackup({ busy: null }); set({ libraryRestoreBusy: false, dataLoading: false }); }
    },
    retryRestoredLibraryRefresh: async () => {
      if (get().portableBackup.busy || !get().portableBackup.refreshFailed) return;
      suspendImages();
      set({ libraryRestoreBusy: true });
      get().closeLightbox();
      patchBackup({ busy: 'refresh', error: null });
      try {
        await runConfigApplyBarrier(() => runAppSettingsApplyBoundary(refreshRestoredLibrary));
        patchBackup({ refreshFailed: false }); get().showToast('恢复后的界面已刷新');
      } catch (error) { reportRefreshFailure(error); }
      finally { resumeImages(); patchBackup({ busy: null }); set({ libraryRestoreBusy: false, dataLoading: false }); }
    },
  };
};


