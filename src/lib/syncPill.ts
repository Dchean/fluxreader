/* TASK-116 X1：侧栏同步 pill 文案的单一事实源（纯函数，回归网直接驱动真值表）。
   优先级契约（Note: 同步四态与失败可见性 — 见 .agents/notes/implemented/feature/2026-10-06-同步四态与失败可见性.md）：
   1. syncStatus === 'error' → 同步失败（不再被「同步中」覆盖——修复既有缺陷：
      旧实现 isBusy 时 syncing 优先级高于 error，手动同步失败态被吞）；
   2. 同步中（手动 syncStatus==='syncing' 或后台 backgroundSyncing）→ 同步中…；
   3. 等待同步 N>0 → 等待同步 N 条（sync_queue 现存行数，本地事实——未连接
      也如实显示，连接后自动补推）；
   4. 其余 → 既有语义逐字保留：后端已同步 / 本地模式 · 直连抓取（X3：无队列
      无失败时不新增噪音）。
   failed>0（队列里存在推送失败过的行）时追加「· 部分失败」段；总长 ≤48 字。 */

import type {
  SubscriptionActionCapability,
  SubscriptionCapabilities,
  SubscriptionOutboxStats,
} from './api';

export interface SyncPillInput {
  syncStatus: 'synced' | 'syncing' | 'error';
  backgroundSyncing: boolean;
  syncConnected: boolean;
  waiting: number;
  failed: number;
}

export function syncPillLabel(s: SyncPillInput): string {
  const base =
    s.syncStatus === 'error'
      ? '同步失败'
      : s.syncStatus === 'syncing' || s.backgroundSyncing
        ? '同步中…'
        : s.waiting > 0
          ? `等待同步 ${s.waiting} 条`
          : s.syncConnected
            ? '后端已同步'
            : '本地模式 · 直连抓取';
  return s.failed > 0 ? `${base} · 部分失败` : base;
}

/* TASK-116 X2：设置页「同步状态」摘要卡 desc 的单一事实源（纯函数，回归网直接
   驱动真值表）。口径（契约 X2/X3 如实原则）：
   - 等待同步 = sync_queue 现存行数；部分失败 = attempts>0 行数（附最新错误
     ≤1 行摘要，按码点截断 60 字符，不劈开代理对）；
   - 「远端已确认」不可累计溯源（成功即 prune 出队），只以「上次同步」时间
     呈现，不虚构总数；
   - 无队列且无失败时不制造噪音（仅时间行 + 说明句）；
   - 收尾固定「本地已保存」说明句：状态变更已事务化落库（TASK-108），
     连接后自动补推。 */
export function syncStateSummary(
  stats: { waiting: number; failed: number; last_error: string | null },
  lastSyncSec: number,
): string {
  const errChars = stats.last_error ? Array.from(stats.last_error) : [];
  const errSummary =
    errChars.length > 60 ? `${errChars.slice(0, 60).join('')}…` : stats.last_error;
  const queueLine = [
    stats.waiting > 0 ? `等待同步 ${stats.waiting} 条` : '',
    stats.failed > 0 ? `部分失败 ${stats.failed}${errSummary ? `（${errSummary}）` : ''}` : '',
  ].filter(Boolean).join('；');
  const lastSyncLine = `上次同步 ${lastSyncSec > 0 ? new Date(lastSyncSec * 1000).toLocaleString() : '从未'}`;
  return [queueLine, lastSyncLine, '状态变更已保存，连接后自动补推'].filter(Boolean).join('；');
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
        `等待 ${stats.pending} 项，其中 ${stats.failed} 项失败${errSummary ? `（${errSummary}）` : ''}`,
      );
    } else {
      parts.push(`等待 ${stats.pending} 项`);
    }
  }
  if (stats.local_only > 0) {
    const proto = protocol === 'fever' ? 'Fever 协议不支持远端写回' : '当前协议不支持远端写回';
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
