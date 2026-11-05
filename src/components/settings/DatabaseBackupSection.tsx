import { useState } from 'react';
import { useAppStore } from '../../store';
import { api, extractError, isTauri } from '../../lib/api';
import type { DatabaseBackupInfo } from '../../lib/api';
import { SettingCard } from '../primitives';

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** 设置中心「关于」页：完整数据库手动备份与恢复边界说明。
 *
 * 备份包含正文与 DPAPI 密文凭据，必须按用户私有数据对待。应用内不提供覆盖式
 * 恢复；恢复要求退出应用后替换主库并移除 WAL/SHM 旁文件，避免半截恢复。
 * Note: 快照保留与恢复边界 — 见 .agents/notes/implemented/architecture/2026-11-05-数据库快照与恢复边界.md
 */
export function DatabaseBackupSection() {
  const showToast = useAppStore((s) => s.showToast);
  const [busy, setBusy] = useState(false);
  const [backup, setBackup] = useState<DatabaseBackupInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  if (!isTauri()) return null;

  const createBackup = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const info = await api.createDatabaseBackup();
      setBackup(info);
      showToast('数据库备份已创建');
    } catch (e) {
      const message = extractError(e);
      setError(`备份失败：${message}`);
      showToast(`备份失败：${message}`);
    } finally {
      setBusy(false);
    }
  };

  const reveal = async () => {
    if (!backup || busy) return;
    setBusy(true);
    try {
      const { revealItemInDir } = await import('@tauri-apps/plugin-opener');
      await revealItemInDir(backup.path);
    } catch (e) {
      const message = extractError(e);
      setError(`无法打开备份文件夹：${message}`);
      showToast(`无法打开备份文件夹：${message}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <div className="settings-group-title" style={{ marginTop: 20 }}>数据备份</div>
      <SettingCard
        title="手动备份"
        desc="创建当前数据库完整快照；备份含正文与加密凭据，请妥善保管"
      >
        <button
          className="toggle-action-btn"
          disabled={busy}
          onClick={() => void createBackup()}
        >
          {busy ? '处理中…' : '创建数据库备份'}
        </button>
      </SettingCard>
      {backup && (
        <div className="settings-action-help database-backup-result" aria-live="polite">
          <div className="database-backup-meta">
            备份已创建 · v{backup.fromVersion} → v{backup.toVersion} · {formatBytes(backup.sizeBytes)}
          </div>
          <code className="database-backup-path">{backup.path}</code>
          <button className="toggle-action-btn" disabled={busy} onClick={() => void reveal()}>
            在文件夹中显示
          </button>
        </div>
      )}
      {error && (
        <div className="settings-action-help database-backup-error" role="alert">
          {error}
        </div>
      )}
      <SettingCard
        title="恢复方式"
        desc="恢复需退出 FluxReader 后替换数据库文件；应用不提供覆盖式恢复"
      />
      <div className="mini-dialog-hint" style={{ marginTop: 8 }}>
        自动快照各保留最近 10 份；手动备份不会被自动清理。
      </div>
    </>
  );
}
