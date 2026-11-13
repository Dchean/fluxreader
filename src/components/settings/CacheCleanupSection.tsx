import { useId, useRef, useState } from 'react';
import { useAppStore } from '../../store';
import { api, extractError } from '../../lib/api';
import { FluxDropdown, SettingCard, ConfirmDialog } from '../primitives';
import { CACHE_PERIODS } from './shared';

/* ---------- 缓存清理（文章 / AI 产物） ---------- */

// Note: 未读永不清理与远端回灌语义 — 见 .agents/notes/implemented/architecture/2026-11-13-身份时间与协议边角.md
export function CacheCleanupSection() {
  const showToast = useAppStore((s) => s.showToast);
  const dataMode = useAppStore((s) => s.dataMode);
  const reloadFromBackend = useAppStore((s) => s.reloadFromBackend);
  const retentionDays = useAppStore((s) => s.settings.readBodyRetentionDays);
  const updateSettings = useAppStore((s) => s.updateSettings);
  const libraryGeneration = useAppStore((s) => s.libraryGeneration);
  const libraryRestoreBusy = useAppStore((s) => s.libraryRestoreBusy);
  const [retentionDraft, setRetentionDraft] = useState<{ text: string; invalid: boolean; generation: number } | null>(null);
  const draft = retentionDraft?.generation === libraryGeneration ? retentionDraft : null;
  const limited = retentionDays > 0 || draft !== null;
  const retentionModeRef = useRef<HTMLDivElement>(null);
  const retentionErrorId = useId();
  const [days, setDays] = useState(30);
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState<'articles' | 'ai' | null>(null);

  if (dataMode !== 'tauri') return null;

  const canSaveRetention = () => {
    const state = useAppStore.getState();
    return !state.libraryRestoreBusy && state.libraryGeneration === libraryGeneration;
  };
  // 草稿只留当前页面；未提交的中间输入不进入设置写队列，远端回读也不覆盖它。
  // 提交即交还store投影，失败由既有原子patch链回滚并提示；成功换库废弃旧草稿。
  const commitRetentionDraft = () => {
    if (!draft || !canSaveRetention()) return;
    const text = draft.text.trim();
    const value = Number(text);
    if (!/^\d+$/.test(text) || !Number.isInteger(value) || value < 1 || value > 3650) {
      setRetentionDraft({ ...draft, invalid: true });
      return;
    }
    setRetentionDraft(null);
    if (value !== useAppStore.getState().settings.readBodyRetentionDays) updateSettings({ readBodyRetentionDays: value });
  };

  const run = async (scope: 'articles' | 'ai') => {
    setBusy(true);
    let cleaned = false;
    try {
      const msg = await api.cacheCleanup(days, scope);
      cleaned = true;
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
      showToast(`${cleaned ? '清理已完成，但界面刷新失败' : '清理失败'}：${extractError(e)}`,
        cleaned ? { label: '重试界面刷新', key: 'cache:refresh', run: () => { void reloadFromBackend().catch(() => {}); } } : undefined);
    } finally {
      setBusy(false);
      setConfirm(null);
    }
  };

  return (
    <>
      <div className="settings-group-title" style={{ marginTop: 20 }}>正文保留</div>
      <SettingCard title="已读正文保留" desc="仅限已读、未收藏正文；未读与收藏的已有正文长期保留">
        <div ref={retentionModeRef} inert={libraryRestoreBusy}>
          <FluxDropdown width={130} ariaLabel="已读正文保留" value={limited ? 'limited' : 'permanent'}
            onChange={(value) => {
              if (!canSaveRetention()) return;
              if (value === 'limited' && limited) { commitRetentionDraft(); return; }
              setRetentionDraft(null);
              const next = value === 'permanent' ? 0 : 30;
              if (next !== useAppStore.getState().settings.readBodyRetentionDays) updateSettings({ readBodyRetentionDays: next });
            }}
            options={[{ value: 'permanent', label: '永久保留' }, { value: 'limited', label: '限定天数' }]} />
        </div>
      </SettingCard>
      {limited && <>
        <SettingCard title="保留天数（1–3650）">
          <input key={libraryGeneration} className="setting-input" style={{ width: 100 }} type="text" inputMode="numeric"
            aria-label="已读正文保留天数" aria-invalid={draft?.invalid || undefined}
            aria-describedby={draft?.invalid ? retentionErrorId : undefined} disabled={libraryRestoreBusy}
            value={draft?.text ?? String(retentionDays || 30)}
            onChange={(event) => setRetentionDraft({ text: event.target.value, invalid: false, generation: libraryGeneration })}
            onBlur={(event) => {
              // 转到模式选择时交由该动作决定，避免先保存草稿再保存永久。
              if (!retentionModeRef.current?.contains(event.relatedTarget)) commitRetentionDraft();
            }}
            onKeyDown={(event) => {
              if (event.key !== 'Enter' || event.nativeEvent.isComposing) return;
              event.preventDefault(); event.stopPropagation(); commitRetentionDraft();
            }} />
        </SettingCard>
        {draft?.invalid && <p id={retentionErrorId} className="settings-action-help" role="alert">请输入 1–3650 的整数天数</p>}
      </>}
      <div className="settings-group-title" style={{ marginTop: 20 }}>缓存清理</div>
      <SettingCard title="按发表时间清理" desc="释放已读、未收藏且无待同步状态的旧正文；保留条目身份和读藏状态">
        <FluxDropdown
          width={110}
          value={String(days)}
          onChange={(v) => setDays(Number(v))}
          options={CACHE_PERIODS.map((p) => ({ value: String(p.days), label: p.label }))}
        />
      </SettingCard>
      <div className="settings-action-row">
        <button className="toggle-action-btn" disabled={busy} onClick={() => setConfirm('articles')}>
          释放旧正文
        </button>
        <button className="toggle-action-btn" disabled={busy} onClick={() => setConfirm('ai')}>
          清理 AI 缓存
        </button>
      </div>
      {/* TASK-102：压缩至 ≤48 字——（收藏除外）/正文保留两条约束保留，
          「重新打开文章可再次生成」为推导性解释 → 删除 */}
      <div className="mini-dialog-hint" style={{ marginTop: 8 }}>
        未读与收藏正文保留；远端仍有的正文可能在同步后重新获取。
        「清理 AI 缓存」仅清摘要与翻译缓存，正文保留。
      </div>
      <ConfirmDialog
        open={confirm !== null}
        title={confirm === 'articles' ? '释放旧正文' : '清理 AI 缓存'}
        message={confirm === 'articles'
          ? `将释放发表于 ${days} 天前、已读、未收藏且无待同步状态的正文，保留条目身份和读藏状态。未读与收藏正文保留；远端仍有的正文可能重新获取。此操作不可撤销。`
          : `将清除发表于 ${days} 天前文章的 AI 摘要与翻译缓存，正文保留。`}
        confirmText="确认清理"
        onConfirm={() => { if (confirm) void run(confirm); }}
        onCancel={() => setConfirm(null)}
      />
    </>
  );
}
