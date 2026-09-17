import { useEffect, useState } from 'react';
import { useAppStore } from '../../store';
import { SettingCard } from '../primitives';
import { openExternal } from '../../lib/external';
import { isComparableVersion, shouldOfferUpdate } from './compareVersions';

/* ---------- TAB 8: 关于 ---------- */

/** 读本地客户端版本；不可用（浏览器预览 / IPC 失败）返回空串。
    不再回退硬编码 '0.8.0'：假版本号会让「检查更新」拿错误的基准去比较。 */
async function resolveLocalVersion(): Promise<string> {
  try {
    const { getVersion } = await import('@tauri-apps/api/app');
    const v = await getVersion();
    return typeof v === 'string' && isComparableVersion(v) ? v : '';
  } catch {
    return '';
  }
}

export function AboutTab() {
  const [version, setVersion] = useState('');
  const [updateState, setUpdateState] = useState<'idle' | 'checking' | 'available' | 'upToDate' | 'failed'>('idle');
  const [latestInfo, setLatestInfo] = useState<{ version: string; url: string } | null>(null);
  const showToast = useAppStore((s) => s.showToast);

  useEffect(() => {
    let alive = true;
    void resolveLocalVersion().then((v) => {
      if (alive && v) setVersion(v);
    });
    return () => { alive = false; };
  }, []);

  const checkUpdate = async () => {
    if (updateState === 'checking') return;
    setUpdateState('checking');
    try {
      /* P2-7：本地版本可能尚未就绪（getVersion 在途或失败）。此时不能比 ——
         compareVersions(remote, '') 把空串当 0.0.0，任何远端版本都判「有更新」。
         先补读一次；仍拿不到就如实报「无法确定」，不给结论。 */
      const local = isComparableVersion(version) ? version : await resolveLocalVersion();
      if (!isComparableVersion(local)) {
        setUpdateState('failed');
        showToast('无法确定本地版本号，请稍后重试');
        return;
      }
      const res = await fetch('https://api.github.com/repos/Dchean/fluxreader/releases/latest');
      if (!res.ok) throw new Error(String(res.status));
      const data = (await res.json()) as { tag_name?: string; html_url?: string };
      const remote = (data.tag_name ?? '').replace(/^v/, '');
      if (!remote) throw new Error('empty tag');
      setLatestInfo({ version: remote, url: data.html_url ?? 'https://github.com/Dchean/fluxreader/releases' });
      setUpdateState(shouldOfferUpdate(remote, local) ? 'available' : 'upToDate');
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
