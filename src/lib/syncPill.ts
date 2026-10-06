/* TASK-116 X1：侧栏同步 pill 文案的单一事实源（纯函数，回归网直接驱动真值表）。
   优先级契约（.workflow-kit/docs/UI-CONTRACT-TASK-116-SYNC-FOUR-STATES.md X1）：
   1. syncStatus === 'error' → 同步失败（不再被「同步中」覆盖——修复既有缺陷：
      旧实现 isBusy 时 syncing 优先级高于 error，手动同步失败态被吞）；
   2. 同步中（手动 syncStatus==='syncing' 或后台 backgroundSyncing）→ 同步中…；
   3. 等待同步 N>0 → 等待同步 N 条（sync_queue 现存行数，本地事实——未连接
      也如实显示，连接后自动补推）；
   4. 其余 → 既有语义逐字保留：后端已同步 / 本地模式 · 直连抓取（X3：无队列
      无失败时不新增噪音）。
   failed>0（队列里存在推送失败过的行）时追加「· 部分失败」段；总长 ≤48 字。 */

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
