import { useEffect, useState } from 'react';
import { useAppStore } from '../../store';
import { SettingCard } from '../primitives';
import { openExternal } from '../../lib/external';
import { compareVersions } from './compareVersions';

/* ---------- TAB 8: 关于 ---------- */

export function AboutTab() {
  const [version, setVersion] = useState('');
  const [updateState, setUpdateState] = useState<'idle' | 'checking' | 'available' | 'upToDate' | 'failed'>('idle');
  const [latestInfo, setLatestInfo] = useState<{ version: string; url: string } | null>(null);
  const showToast = useAppStore((s) => s.showToast);

  useEffect(() => {
    let alive = true;
    import('@tauri-apps/api/app')
      .then(({ getVersion }) => getVersion())
      .then((v) => alive && setVersion(v))
      .catch(() => alive && setVersion('0.8.0'));
    return () => { alive = false; };
  }, []);

  const checkUpdate = async () => {
    if (updateState === 'checking') return;
    setUpdateState('checking');
    try {
      const res = await fetch('https://api.github.com/repos/Dchean/fluxreader/releases/latest');
      if (!res.ok) throw new Error(String(res.status));
      const data = (await res.json()) as { tag_name?: string; html_url?: string };
      const remote = (data.tag_name ?? '').replace(/^v/, '');
      if (!remote) throw new Error('empty tag');
      setLatestInfo({ version: remote, url: data.html_url ?? 'https://github.com/Dchean/fluxreader/releases' });
      setUpdateState(compareVersions(remote, version) > 0 ? 'available' : 'upToDate');
    } catch {
      setUpdateState('failed');
      showToast('检查更新失败，请稍后重试');
    }
  };

  return (
    <>
      <SettingCard title="客户端版本" desc={`FluxReader v${version || '…'} (Build 2026.08)`}>
        <span className="about-arch-tag">Tauri 2 + Rust + SQLite</span>
      </SettingCard>
      <SettingCard title="检查更新" desc="检测 GitHub Releases 上的最新版本">
        {updateState === 'checking' ? (
          <span className="about-update-hint">正在检查…</span>
        ) : updateState === 'available' && latestInfo ? (
          <button className="toggle-action-btn about-update-btn" onClick={() => void openExternal(latestInfo.url)}>
            v{latestInfo.version} 可用 · 前往下载
          </button>
        ) : (
          <button className="toggle-action-btn about-update-btn" onClick={() => void checkUpdate()}>
            检查更新
          </button>
        )}
        {updateState === 'upToDate' && <span className="about-update-hint">已是最新版本</span>}
        {updateState === 'failed' && <span className="about-update-hint">检查失败</span>}
      </SettingCard>
    </>
  );
}
