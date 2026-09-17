import { useEffect, useState } from 'react';
import { useAppStore } from '../../store';
import { api, extractError } from '../../lib/api';
import { openExternal } from '../../lib/external';
import { FluxDropdown, SettingCard } from '../primitives';

/* ---------- 配置同步（GitHub Gist / WebDAV）---------- */

export function ConfigSyncSection() {
  const showToast = useAppStore((s) => s.showToast);
  const reloadFromBackend = useAppStore((s) => s.reloadFromBackend);
  const bootstrapSettings = useAppStore((s) => s.bootstrapSettings);
  const [backend, setBackend] = useState<'gist' | 'webdav'>('gist');
  const [token, setToken] = useState('');
  const [server, setServer] = useState('');
  const [username, setUsername] = useState('');
  const [status, setStatus] = useState<{ configured: boolean; backend?: string; lastUpload?: string } | null>(null);
  const [busy, setBusy] = useState(false);

  /* ---- GitHub 设备流登录态（状态与轮询在 store 层，组件只做展示触发） ---- */
  const ghAccount = useAppStore((s) => s.githubAccount);
  const ghFlow = useAppStore((s) => s.githubFlow);
  const ghLoggingIn = useAppStore((s) => s.githubLoggingIn);
  const githubLoginStart = useAppStore((s) => s.githubLoginStart);
  const githubLoginDisconnect = useAppStore((s) => s.githubLoginDisconnect);

  const doGhLogin = () => void githubLoginStart();
  const doGhDisconnect = () => {
    void githubLoginDisconnect().then(() => {
      setStatus(null);
    });
  };

  /* 挂载 + GitHub 登录态变化时同步后端配置状态（登录后上传入口立即可用） */
  useEffect(() => {
    void api.configSyncStatus().then((st) => {
      if (st) {
        setStatus(st);
        if (st.backend === 'webdav') setBackend('webdav');
      }
    });
  }, [ghAccount]);

  const doSave = async () => {
    if (!token.trim()) {
      showToast(backend === 'gist' ? '请填写 GitHub Token（classic PAT，勾选 gist 权限）' : '请填写 WebDAV 密码');
      return;
    }
    if (backend === 'webdav' && (!server.trim() || !username.trim())) {
      showToast('请填写 WebDAV 服务器地址与用户名');
      return;
    }
    setBusy(true);
    try {
      await api.configSyncSaveCredentials(JSON.stringify({
        backend, token: token.trim(), server: server.trim(), username: username.trim(), gist_id: null,
      }));
      setStatus({ configured: true, backend });
      setToken('');
      if (backend === 'gist') {
        /* PAT 手动保存后登录态按未知账户处理（手动模式不显示账户名，只显示已配置） */
        useAppStore.setState({ githubAccount: null });
      }
      showToast('凭据已保存（本地存储）');
    } catch (e) {
      showToast(`保存失败：${extractError(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const doUpload = async () => {
    setBusy(true);
    try {
      const at = await api.configSyncUpload();
      setStatus({ configured: true, backend, lastUpload: at });
      showToast('配置已上传');
    } catch (e) {
      showToast(`上传失败：${extractError(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const doDownload = async () => {
    setBusy(true);
    try {
      const payload = await api.configSyncDownload();
      const parsed = JSON.parse(payload) as { uploaded_at?: string; feeds?: unknown[] };
      const n = Array.isArray(parsed.feeds) ? parsed.feeds.length : 0;
      if (!window.confirm(`下载远端配置（${n} 个订阅源，上传于 ${parsed.uploaded_at ?? '未知时间'}）并应用？\n\n已存在的源会跳过；本地设置与 AI 配置将被远端覆盖。`)) {
        setBusy(false);
        return;
      }
      const r = await api.configSyncApply(payload);
      await reloadFromBackend();
      await bootstrapSettings().catch(() => null);
      showToast(`配置已应用：新增 ${r.imported} 个源${r.skipped > 0 ? `，跳过 ${r.skipped} 个已存在` : ''}`);
    } catch (e) {
      showToast(`下载失败：${extractError(e)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <div className="settings-group-title" style={{ marginTop: 20 }}>GitHub 同步</div>
      {ghAccount ? (
        <SettingCard
          title="已登录 GitHub"
          desc={`账户 ${ghAccount.login} · 配置经私有 Gist 同步（与 PAT 等效，仅本机存储 token）`}
        >
          <button className="toggle-action-btn btn-danger-text" disabled={ghLoggingIn} onClick={() => void doGhDisconnect()}>
            断开
          </button>
        </SettingCard>
      ) : ghFlow ? (
        <SettingCard
          title="等待授权"
          desc="在浏览器打开授权页，输入下方代码完成授权。此卡片常驻，登录成功后自动消失"
        >
          <div className="device-code-block">
            <span className="device-code-value">{ghFlow.user_code}</span>
            <span className="device-code-hint">在授权页输入该代码</span>
          </div>
          <button
            className="toggle-action-btn"
            onClick={() => void openExternal(ghFlow.verification_uri)}
            title="重新打开授权页"
          >
            打开授权页
          </button>
        </SettingCard>
      ) : (
        <SettingCard
          title="网页登录 GitHub"
          desc="跳转浏览器完成 GitHub 授权，登录后配置同步到你的私有 Gist"
        >
          <button className="toggle-action-btn btn-primary" disabled={ghLoggingIn} onClick={() => void doGhLogin()}>
            {ghLoggingIn ? '发起中…' : '登录 GitHub'}
          </button>
        </SettingCard>
      )}

      <div className="settings-group-title" style={{ marginTop: 20 }}>配置同步（Gist / WebDAV）</div>
      <SettingCard
        title="同步内容与状态"
        desc={status?.configured
          ? `已配置（${status.backend === 'webdav' ? 'WebDAV' : 'GitHub Gist'}）${status.lastUpload ? ` · 上次上传 ${new Date(status.lastUpload).toLocaleString()}` : ' · 从未上传'}`
          : '同步订阅源结构、分类布局、AI 标志、界面设置与 AI 配置（不含正文与 AI 缓存）'}
      >
        <span className="about-arch-tag">{status?.configured ? '已配置' : '未配置'}</span>
      </SettingCard>
      <SettingCard title="同步后端" desc="Gist 可用上方网页登录自动配置，或手动填 classic PAT；WebDAV 填服务器地址与账号">
        <FluxDropdown
          width={140}
          value={backend}
          onChange={(v) => setBackend(v as 'gist' | 'webdav')}
          options={[
            { value: 'gist', label: 'GitHub Gist' },
            { value: 'webdav', label: 'WebDAV' },
          ]}
        />
      </SettingCard>
      {backend === 'gist' ? (
        <SettingCard title="GitHub Token（classic PAT）" desc="手动填入替代网页登录；需 classic PAT（勾选 gist scope），fine-grained PAT 不支持 Gist API">
          <input
            type="password"
            className="setting-input"
            placeholder="ghp_…"
            value={token}
            onChange={(e) => setToken(e.target.value)}
          />
        </SettingCard>
      ) : (
        <>
          <SettingCard title="WebDAV 服务器" desc="例如 https://dav.example.com/fluxreader（配置存为 fluxreader-config.json）">
            <input
              type="text"
              className="setting-input"
              placeholder="https://dav.example.com/path"
              value={server}
              onChange={(e) => setServer(e.target.value)}
            />
          </SettingCard>
          <SettingCard title="WebDAV 用户名" desc="服务器登录账号">
            <input
              type="text"
              className="setting-input"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
            />
          </SettingCard>
          <SettingCard title="WebDAV 密码" desc="应用专用密码更安全（坚果云等用应用密码）">
            <input
              type="password"
              className="setting-input"
              value={token}
              onChange={(e) => setToken(e.target.value)}
            />
          </SettingCard>
        </>
      )}
      <div className="settings-action-row">
        <button className="toggle-action-btn" disabled={busy} onClick={() => void doSave()}>
          保存凭据
        </button>
        <button className="toggle-action-btn btn-primary" disabled={busy || !status?.configured} onClick={() => void doUpload()}>
          {busy ? '处理中…' : '上传配置'}
        </button>
        <button className="toggle-action-btn" disabled={busy || !status?.configured} onClick={() => void doDownload()}>
          下载并应用
        </button>
      </div>
      <div className="mini-dialog-hint" style={{ marginTop: 8 }}>
        手动上传/下载模式：下载会覆盖本地设置与 AI 配置，订阅源按 URL 合并（已存在跳过）。
        多设备使用时，换机先「上传」，新机「下载并应用」。
      </div>
    </>
  );
}
