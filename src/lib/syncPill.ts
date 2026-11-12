/* TASK-116 X1：侧栏同步指示器的单一事实源（纯函数，回归网直接驱动真值表）。
   2026-11-04 起，常驻文案不再承载队列数量：
   1. 手动/后台同步中 → 同步中（旧失败详情收进 tooltip/aria）；
   2. 空闲时 syncStatus === 'error' 或任一失败计数 > 0 → 同步失败；
   3. 无任何待处理项且处于完成后的短窗口 → 同步完成；
   4. 其余显示同步账户名；未连接/无账户显示「本地模式」。
   数量只进 tooltip / aria / 设置摘要；圆点颜色由 tone 收口（pending 橙、
   success 绿、error 红、local 灰、syncing 橙并呼吸）。 */

import type {
  SubscriptionActionCapability,
  SubscriptionCapabilities,
  SubscriptionOutboxStats,
  LocalSyncStatus,
} from './api';

export interface SyncPillInput {
  syncStatus: 'synced' | 'syncing' | 'error';
  backgroundSyncing: boolean;
  syncConnected: boolean;
  account?: string | null;
  protocol?: string | null;
  lastSync?: number;
  waiting: number;
  failed: number;
  localOnly?: number;
  bindableRetry?: number;
  expiredUnconfirmed?: number;
  /** 订阅写回 outbox（当前账号限定）；null/缺失 = 未加载，不冒充无待办。 */
  subscription?: SubscriptionOutboxStats | null;
  /** 手动同步/保存并同步成功后的时间戳；仅短窗口内显示「同步完成」。 */
  succeededAt?: number | null;
  /** 最近一次硬失败摘要；只进 tooltip / aria / live region。 */
  lastError?: string | null;
  /** 注入时钟，供纯函数测试短窗口边界。 */
  now?: number;
  /** 仅纯本地模式采用；undefined=旧调用方未传，null=状态尚未确认。 */
  localSync?: LocalSyncStatus | null;
  localSyncBusy?: 'connect' | 'disconnect' | 'run' | null;
  localSyncError?: string | null;
}

export const SYNC_SUCCESS_LABEL_MS = 2_400;

export type SyncIndicatorTone = 'local' | 'pending' | 'syncing' | 'success' | 'error';

export interface SyncPillView {
  label: string;
  tone: SyncIndicatorTone;
  busy: boolean;
  /** 状态详情（tooltip / aria）；常驻可见文案不包含数量。 */
  details: string;
  title: string;
  /** 圆点按钮的可访问名称（动作 + 账户/详情）。 */
  syncAriaLabel: string;
  /** 文字按钮的可访问名称（打开设置）。 */
  settingsAriaLabel: string;
  /** 隐藏 live region 的播报文本。 */
  announcement: string;
}

// Note: 常驻文案/灯色是纯展示状态机，后端 sync_status + 队列/outbox 才是事实源 — 见 .agents/notes/implemented/feature/2026-11-04-同步灯与账户状态.md
function syncProtocolLabel(protocol: string | null | undefined): string | null {
  if (protocol === 'greader') return 'GReader';
  if (protocol === 'fever') return 'Fever';
  return null;
}

export function syncPillView(s: SyncPillInput): SyncPillView {
  if (!s.syncConnected && s.localSync !== undefined
    && (s.localSync === null || s.localSync.connected || s.localSyncBusy || s.localSyncError)) {
    return localSyncPillView(s.localSync, s.localSyncBusy ?? null, s.localSyncError ?? null);
  }
  const now = s.now ?? Date.now();
  const subscription = s.subscription ?? null;
  const subscriptionUnknown = subscription == null;
  const localOnly = (s.localOnly ?? 0) + (subscription?.local_only ?? 0);
  const bindableRetry = s.bindableRetry ?? 0;
  const expiredUnconfirmed = s.expiredUnconfirmed ?? 0;
  const subscriptionPending = subscription?.pending ?? 0;
  const failureCount =
    s.failed
    + (subscription?.failed ?? 0);
  const pendingCount =
    s.waiting
    + localOnly
    + bindableRetry
    + expiredUnconfirmed
    + subscriptionPending;
  const busy = s.syncStatus === 'syncing' || s.backgroundSyncing;
  const account = s.account?.trim() ?? '';
  const accountVerified = s.syncConnected && account !== '';
  const recentlySucceeded =
    !busy
    && s.syncStatus !== 'error'
    && failureCount === 0
    && pendingCount === 0
    && !subscriptionUnknown
    && accountVerified
    && s.succeededAt != null
    && now >= s.succeededAt
    && now - s.succeededAt < SYNC_SUCCESS_LABEL_MS;
  const failed = !busy && (s.syncStatus === 'error' || failureCount > 0);
  const accountLabel = s.syncConnected && account ? account : '本地模式';
  const protocol = syncProtocolLabel(s.protocol);
  const errorTexts: string[] = [];
  for (const text of [s.lastError, subscription?.last_error]) {
    const trimmed = text?.trim();
    if (trimmed && !errorTexts.includes(trimmed)) errorTexts.push(trimmed);
  }
  const errorText = errorTexts.join('；');

  const detailParts = [
    s.waiting > 0 ? `状态待推送 ${s.waiting} 条` : '',
    subscriptionPending > 0 ? `订阅变更待推送 ${subscriptionPending} 项` : '',
    localOnly > 0 ? `仅本机 ${localOnly} 条` : '',
    bindableRetry > 0 ? `等待绑定 ${bindableRetry} 条` : '',
    expiredUnconfirmed > 0 ? `过期未确认 ${expiredUnconfirmed} 条` : '',
    failureCount > 0 ? `失败 ${failureCount} 条` : '',
    subscriptionUnknown ? '订阅状态未加载（未知）' : '',
    errorText ? `最近错误：${errorText}` : '',
  ].filter(Boolean);
  const details = detailParts.length > 0 ? detailParts.join('；') : '当前没有待同步项';
  const connectionLine = s.syncConnected
    ? `${account ? `同步账户 ${account}` : '同步服务已配置 · 账户缺失（未验证）'}${protocol ? ` · ${protocol}` : ''}`
      + ` · ${(s.lastSync ?? 0) > 0 ? `上次同步 ${new Date((s.lastSync ?? 0) * 1000).toLocaleString()}` : '上次同步 从未'}`
    : '本地模式 · 未连接同步服务';
  const title = `${connectionLine}；${details}。点击圆点手动同步，点击账户打开同步设置。`;
  const label = busy
    ? '同步中'
    : failed
      ? '同步失败'
      : recentlySucceeded
        ? '同步完成'
        : accountLabel;
  const tone: SyncIndicatorTone = busy
    ? 'syncing'
    : failed
      ? 'error'
      : recentlySucceeded
        ? 'success'
        : pendingCount > 0 || (subscriptionUnknown && accountVerified)
          ? 'pending'
          : s.syncConnected && account
            ? 'success'
            : 'local';

  return {
    label,
    tone,
    busy,
    details,
    title,
    syncAriaLabel: busy
      ? `正在同步，手动同步按钮暂不可用。${title}`
      : `手动同步。${title}`,
    settingsAriaLabel: `打开同步设置。当前状态：${label}。${title}`,
    announcement: `${label}。${details}`,
  };
}

/** WebDAV的确认事实来自独立状态/正文通道，不借用RSS活动的成功绿灯。 */
function localSyncPillView(status: LocalSyncStatus | null, activity: SyncPillInput['localSyncBusy'], error: string | null): SyncPillView {
  const busy = activity != null;
  const failure = error || status?.last_error;
  const pending = status ? status.pending_operations + status.pending_bodies + status.pending_publications : 0;
  const confirmed = status != null && status.last_state_sync > 0 && status.last_body_sync > 0;
  const label = busy ? activity === 'connect' ? '连接中' : activity === 'disconnect' ? '断开中' : '状态同步中'
    : failure ? '同步未完成' : status?.connected ? 'WebDAV' : status === null ? '同步状态未确认' : '本地模式';
  const tone: SyncIndicatorTone = busy ? 'syncing' : failure ? 'error'
    : !status || (status.connected && (!confirmed || pending > 0)) ? 'pending'
      : status.connected && confirmed ? 'success' : 'local';
  const time = (seconds: number) => seconds > 0 ? new Date(seconds * 1000).toLocaleString() : '尚未同步';
  const details = status ? [
    status.connected ? `WebDAV 已配置连接${status.username ? ` · ${status.username}` : ''}` : '本地模式 · WebDAV 未连接',
    `状态确认：${time(status.last_state_sync)}`,
    `正文确认：${time(status.last_body_sync)}`,
    `待处理状态 ${status.pending_operations} 项`,
    `待确认批次 ${status.pending_publications} 个`,
    `本机待补正文 ${status.pending_bodies} 篇`,
    status.next_retry_at > 0 ? `下次重试：${time(status.next_retry_at)}` : '',
    failure ? `最近错误：${failure}` : '',
  ].filter(Boolean).join('；') : `连接与同步状态尚未确认${failure ? `；${failure}` : ''}`;
  const title = `${details}。点击圆点${status?.connected ? '同步 WebDAV' : '刷新或检查连接'}，点击文字打开同步设置。`;
  return { label, tone, busy, details, title,
    syncAriaLabel: busy ? `操作进行中，手动同步按钮暂不可用。${title}` : `手动同步。${title}`,
    settingsAriaLabel: `打开同步设置。当前状态：${label}。${title}`,
    announcement: `${label}。${details}` };
}

export function syncPillLabel(s: SyncPillInput): string {
  return syncPillView(s).label;
}

/* TASK-116 X2：设置页「同步状态」摘要卡 desc 的单一事实源（纯函数，回归网直接
   驱动真值表）。口径（契约 X2/X3 如实原则）：
   - 状态待推送 = 可推送未确认项；失败 = 终态 failed + 独立传入的
     retry_wait（附最新错误 ≤1 行摘要，按码点截断 60 字符，不劈开代理对）；
   - 「远端已确认」不可累计溯源（成功即 prune 出队），只以「上次同步」时间
     呈现，不虚构总数；
   - 无队列且无失败时不制造噪音（仅时间行 + 说明句）；
   - local_only / bindable_retry / expired_unconfirmed 必须单独显示，不能
     用「连接后自动补推」掩盖（这些意图当前无远端身份）；
   - 终态 failed 不得承诺「连接后自动补推」；`target_missing` 已由 ACK
     读回写入终态，只有用户主动重试（full states 阶段重排）才可能恢复，
     纯函数仍按终态如实呈现；
   - 收尾说明句按是否仍有未同步意图分流，始终只承诺已经成立的事实。 */
export function syncStateSummary(
  stats: {
    waiting: number;
    failed: number;
    local_only?: number;
    bindable_retry?: number;
    retry_wait?: number;
    expired_unconfirmed?: number;
    last_error: string | null;
  },
  lastSyncSec: number,
): string {
  const errChars = stats.last_error ? Array.from(stats.last_error) : [];
  const errSummary =
    errChars.length > 60 ? `${errChars.slice(0, 60).join('')}…` : stats.last_error;
  const retryWait = stats.retry_wait ?? 0;
  const failureCount = stats.failed + retryWait;
  const localOnly = stats.local_only ?? 0;
  const bindableRetry = stats.bindable_retry ?? 0;
  const expired = stats.expired_unconfirmed ?? 0;
  const unresolved = localOnly + bindableRetry + expired;
  const queueLine = [
    stats.waiting > 0 ? `状态待推送 ${stats.waiting} 条` : '',
    failureCount > 0
      ? `失败 ${failureCount}${errSummary ? `（${errSummary}）` : ''}`
      : '',
    localOnly > 0 ? `仅本机 ${localOnly} 条` : '',
    bindableRetry > 0 ? `等待绑定 ${bindableRetry} 条` : '',
    expired > 0 ? `过期未确认 ${expired} 条` : '',
  ].filter(Boolean).join('；');
  const lastSyncLine = `上次同步 ${lastSyncSec > 0 ? new Date(lastSyncSec * 1000).toLocaleString() : '从未'}`;
  const savedLine =
    stats.failed > 0
      ? unresolved > 0
        ? '状态变更已保存；终态失败项不会自动重试，未同步项保留在本机'
        : '状态变更已保存；终态失败项不会自动重试'
      : unresolved > 0
        ? '状态变更已保存；未同步项保留在本机'
        : '状态变更已保存，连接后自动补推';
  return [queueLine, lastSyncLine, savedLine].filter(Boolean).join('；');
}

/* ============================================================
   OPT-009 B3：订阅写回的呈现纯函数（单一事实源；真值表由
   tools/frontend-tests/subscription-writeback.mjs 驱动）。
   ============================================================ */

/** 订阅写回回执摘要行（追加在同步摘要卡 desc）。口径：
 * - 无待办（pending=0 且 local_only=0）→ 空串：不新增噪音行（X3 口径）；
 * - pending → 「等待 N 项」；failed（pending 子集）→ 追加「其中 M 项失败
 *   （最近错误，按码点截断 60，不劈代理对）」；
 * - local_only → 必须如实点名「仅本地生效」并携带协议事实（Fever ≠ 已同步；
 *   不得写成成功，也不得隐藏）。 */
export function subscriptionWriteSummary(
  stats: SubscriptionOutboxStats,
  protocol: 'greader' | 'fever' | null,
): string {
  const parts: string[] = [];
  if (stats.pending > 0) {
    if (stats.failed > 0) {
      const errChars = stats.last_error ? Array.from(stats.last_error) : [];
      const errSummary =
        errChars.length > 60 ? `${errChars.slice(0, 60).join('')}…` : stats.last_error;
      parts.push(
        `订阅变更待推送 ${stats.pending} 项，其中 ${stats.failed} 项失败${errSummary ? `（${errSummary}）` : ''}`,
      );
    } else {
      parts.push(`订阅变更待推送 ${stats.pending} 项`);
    }
  }
  if (stats.local_only > 0) {
    const proto = protocol === 'fever' ? 'Fever 不支持订阅写回' : '当前协议不支持订阅写回';
    parts.push(`${stats.local_only} 项仅本地生效（${proto}）`);
  }
  return parts.length > 0 ? `订阅写回：${parts.join('；')}` : '';
}

/** 能力矩阵逐动作投影（能力卡一行）：支持 → 已支持 / 仅本地 / 不支持。 */
export function subscriptionActionLine(caps: SubscriptionCapabilities): string {
  const label = (c: SubscriptionActionCapability): string =>
    c.support === 'Supported' ? '已支持' : c.support === 'LocalOnly' ? '仅本地' : '不支持';
  return [
    `新增 ${label(caps.subscribe)}`,
    `编辑 ${label(caps.edit_title)}`,
    `移动 ${label(caps.move_folder)}`,
    `退订 ${label(caps.unsubscribe)}`,
    `分类改名 ${label(caps.rename_folder)}`,
    `分类删除 ${label(caps.delete_folder)}`,
    `空分类 ${label(caps.empty_folder_remote)}`,
  ].join(' · ');
}

/** 删除确认的远端语义句——唯一直取 subscription_capabilities（与能力矩阵同源，
 *  不在 UI 另写一套说法）。分类**有成员**取 delete_folder.detail（成员逐源退订）、
 *  **空分类**取 empty_folder_remote.detail（无可验证的远端表达）。
 *  caps 未加载 → null：调用方省略该句（不编造远端语义）。 */
export function deleteRemoteLine(
  caps: SubscriptionCapabilities | null,
  kind: 'feed' | 'folder',
  memberCount: number,
): string | null {
  if (!caps) return null;
  if (kind === 'feed') return caps.unsubscribe.detail;
  return memberCount > 0 ? caps.delete_folder.detail : caps.empty_folder_remote.detail;
}
