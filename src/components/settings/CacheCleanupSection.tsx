import { useState } from 'react';
import { useAppStore } from '../../store';
import { api, extractError } from '../../lib/api';
import { FluxDropdown, SettingCard, ConfirmDialog } from '../primitives';
import { CACHE_PERIODS } from './shared';

/* ---------- 缓存清理（文章 / AI 产物） ---------- */

export function CacheCleanupSection() {
  const showToast = useAppStore((s) => s.showToast);
  const dataMode = useAppStore((s) => s.dataMode);
  const reloadFromBackend = useAppStore((s) => s.reloadFromBackend);
  const [days, setDays] = useState(30);
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState<'articles' | 'ai' | null>(null);

  if (dataMode !== 'tauri') return null;

  const run = async (scope: 'articles' | 'ai') => {
    setBusy(true);
    try {
      const msg = await api.cacheCleanup(days, scope);
      showToast(msg ?? '清理完成');
      /* TASK-122：清理动作完成后必须 reload——AI 缓存清理（scope='ai'）在后端把
         ai_summary/translated_content 置 NULL，前端对受影响 id 的显式失效
         （bodyById 记录置 cleared + bump contentRevision）由这次 reload 落地时的
         mergeSnapshotEntries(fromBackend=true) → bodyCache.reconcileBodyEntities
         按**行真值**逐 id 执行（前端无法预知清理范围，行 NULL 即删除语义；
         机制与状态机见 store/bodyCache.ts 模块头注「显式失效规则」单点文档）。
         清理后如有在途水合/reload，cleared 态不会被复活（行对齐跳过 cleared +
         失效戳比对丢弃迟到响应）。 */
      await reloadFromBackend();
    } catch (e) {
      showToast(`清理失败：${extractError(e)}`);
    } finally {
      setBusy(false);
      setConfirm(null);
    }
  };

  return (
    <>
      <div className="settings-group-title" style={{ marginTop: 20 }}>缓存清理</div>
      <SettingCard title="清理时间范围" desc="删除该时间之前的本地缓存（收藏文章与待同步状态始终保留）">
        <FluxDropdown
          width={110}
          value={String(days)}
          onChange={(v) => setDays(Number(v))}
          options={CACHE_PERIODS.map((p) => ({ value: String(p.days), label: p.label }))}
        />
      </SettingCard>
      <div className="settings-action-row">
        <button className="toggle-action-btn" disabled={busy} onClick={() => setConfirm('articles')}>
          清理旧文章
        </button>
        <button className="toggle-action-btn" disabled={busy} onClick={() => setConfirm('ai')}>
          清理 AI 缓存
        </button>
      </div>
      {/* TASK-102：压缩至 ≤48 字——（收藏除外）/正文保留两条约束保留，
          「重新打开文章可再次生成」为推导性解释 → 删除 */}
      <div className="mini-dialog-hint" style={{ marginTop: 8 }}>
        「清理旧文章」删指定时间前文章（收藏除外）；「清理 AI 缓存」仅清摘要与翻译缓存，正文保留。
      </div>
      <ConfirmDialog
        open={confirm !== null}
        title={confirm === 'articles' ? '清理旧文章' : '清理 AI 缓存'}
        message={confirm === 'articles'
          ? `将删除 ${CACHE_PERIODS.find((p) => p.days === days)?.label ?? `${days} 天`} 之前的文章（收藏与待同步项保留），此操作不可撤销。`
          : `将清除 ${CACHE_PERIODS.find((p) => p.days === days)?.label ?? `${days} 天`} 之前文章的 AI 摘要与翻译缓存，正文保留。`}
        confirmText="确认清理"
        onConfirm={() => { if (confirm) void run(confirm); }}
        onCancel={() => setConfirm(null)}
      />
    </>
  );
}
