export type AddFeedSyncProtocol = 'greader' | 'fever' | null;

/** AddFeed 的同步说明只按当前状态给一句，避免 Fever/未连接继承 GReader 承诺。 */
export function addFeedSyncHint(
  connected: boolean,
  protocol: AddFeedSyncProtocol,
): string {
  if (!connected) {
    return '离线也能添加并直接抓取；连接同步服务后，可在「同步本地订阅」中确认是否推送。';
  }
  if (protocol === 'fever') {
    return 'Fever：新增订阅仅本地保存，同步服务不会改变。';
  }
  if (protocol === 'greader') {
    return '勾选「同步到账户」只会添加本次订阅，不推送已有本地订阅。';
  }
  return '同步协议尚未确认：新增订阅只保存在本机，请到同步页确认连接状态。';
}

/** 只有已连接 GReader 才展示“同步到账户”开关。 */
export function addFeedShowsSyncSwitch(
  connected: boolean,
  protocol: AddFeedSyncProtocol,
): boolean {
  return connected && protocol === 'greader';
}

/** 已连接但能力矩阵未加载时，删除确认必须如实说明账户影响未知。 */
export function unknownDeleteImpactWarning(
  connected: boolean,
  capabilitiesLoaded: boolean,
): string | null {
  return connected && !capabilitiesLoaded
    ? '当前无法读取同步服务能力，无法确认此操作对同步账户的影响，请稍后重试或仅按本地后果继续'
    : null;
}

export function searchErrorVisible(query: string, searchError: boolean): boolean {
  return query.trim() !== '' && searchError;
}

export const EXTERNAL_OPEN_FAILURE =
  '无法在浏览器中打开此链接，请检查默认浏览器设置或稍后重试。';

export const AUDIO_FAILURE_DEDUPE_MS = 3000;

/** 全刷入口必须如实描述源集合：hybrid 只抓本地源，direct 才抓全部来源。 */
export function refreshAllLabel(syncMode: 'direct' | 'hybrid'): string {
  return syncMode === 'hybrid' ? '刷新本地订阅源' : '刷新全部订阅源';
}

export type AudioFailureSnapshot = { key: string; at: number } | null;

export function audioFailureKey(url: string): string {
  return `audio:play:${url}`;
}

/** 同一音频的 play reject 与 error 事件可能同时到达：窗口内只报一次。 */
export function shouldReportAudioFailure(
  previous: AudioFailureSnapshot,
  key: string,
  now: number,
  dedupeMs: number = AUDIO_FAILURE_DEDUPE_MS,
): boolean {
  return previous === null || previous.key !== key || now - previous.at >= dedupeMs;
}
