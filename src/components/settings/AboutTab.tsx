import { useEffect, useState } from 'react';
import { useAppStore } from '../../store';
import { SettingCard } from '../primitives';
import { openExternal } from '../../lib/external';
import { api, extractError } from '../../lib/api';
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
      /* OPT-014 / F16：出网检查收口到 Rust 固定目的地命令——前端不再直接
         fetch GitHub，生产 CSP 的 connect-src 不放宽，webview 也没有任意
         URL 代理能力；下载地址已由后端限制在本仓库 releases 域。
         远端 tag 不可比较（异常响应）时按失败处理，不误报「已是最新」。
         Note: 见 .agents/notes/implemented/architecture/2026-10-08-凭据失败关闭与受控更新检查.md */
      const res = await api.checkForUpdates();
      if (!res || !isComparableVersion(res.version)) throw new Error('响应缺少可用版本号');
      setLatestInfo({ version: res.version, url: res.url });
      setUpdateState(shouldOfferUpdate(res.version, local) ? 'available' : 'upToDate');
    } catch (e) {
      setUpdateState('failed');
      showToast(`检查更新失败：${extractError(e)}`);
    }
  };

  return (
    <>
      <SettingCard title="客户端版本" desc={`FluxReader v${version || '…'}`}>
        <span className="about-arch-tag">Tauri 2 + Rust + SQLite</span>
      </SettingCard>
      {/* TASK-102：desc 复述「检查更新」标题 → 删除（按钮文案已自解释） */}
      <SettingCard title="检查更新">
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
