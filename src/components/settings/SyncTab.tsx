import { useEffect, useState } from 'react';
import { useAppStore } from '../../store';
import { api, extractError } from '../../lib/api';
import { FluxDropdown, Switch, SettingCard, ConfirmDialog } from '../primitives';
import { CacheCleanupSection } from './CacheCleanupSection';
import { ConfigSyncSection } from './ConfigSyncSection';
import { ENDPOINT_DESC, ENDPOINT_PLACEHOLDER, endpointHint } from './endpointHint';

/* ---------- TAB 6: 同步 ---------- */

export function SyncTab() {
  const showToast = useAppStore((s) => s.showToast);
  const dataMode = useAppStore((s) => s.dataMode);
  const reloadFromBackend = useAppStore((s) => s.reloadFromBackend);
  const settings = useAppStore((s) => s.settings);
  const updateSettings = useAppStore((s) => s.updateSettings);
  const [endpoint, setEndpoint] = useState('');
  const [protocol, setProtocol] = useState<'greader' | 'fever'>('greader');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [connected, setConnected] = useState(false);
  const [account, setAccount] = useState<string | null>(null);
  const [lastSync, setLastSync] = useState(0);
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [confirmDisconnect, setConfirmDisconnect] = useState(false);
  /** 首连弹窗：本地未绑定源数（>0 弹「同步本地订阅到后端」确认） */
  const [pendingLocalSync, setPendingLocalSync] = useState(0);
  const [syncingLocal, setSyncingLocal] = useState(false);

  /* 打开设置时读取当前连接状态 */
  useEffect(() => {
    if (dataMode !== 'tauri') return;
    void api.syncStatus().then((st) => {
      if (!st) return;
      setConnected(st.connected);
      setAccount(st.account);
      setLastSync(st.last_sync);
      if (st.connected && st.endpoint) setEndpoint(st.endpoint);
      setProtocol(st.protocol === 'fever' ? 'fever' : 'greader');
    });
  }, [dataMode]);

  /* 轻量连通测试：不落库不做同步（填表时快速验证） */
  const doTest = async () => {
    if (!endpoint.trim() || !username.trim() || !password.trim()) {
      showToast('请填写 Endpoint、用户名和密码');
      return;
    }
    setTesting(true);
    try {
      const msg = await api.syncTest(protocol, endpoint.trim(), username.trim(), password.trim());
      showToast(msg ?? '连接成功');
    } catch (e) {
      showToast(`连接失败：${endpointHint(extractError(e))}`);
    } finally {
      setTesting(false);
    }
  };

  /** 保存并后台同步：保存秒回（只做轻量测试+落库），
      订阅/状态的拉取全部后台执行，设置页可随时关闭。
      已连接且 Token 留空 = 复用已存 Token（仅改 Endpoint） */
  const doSaveAndSync = async () => {
    if (!endpoint.trim()) {
      showToast('请填写 Endpoint');
      return;
    }
    if ((!username.trim() || !password.trim()) && !connected) {
      showToast('请填写用户名和密码');
      return;
    }
    setSaving(true);
    try {
      const result = await api.syncSave(protocol, endpoint.trim(), username.trim(), password.trim());
      setConnected(true);
      /* 保存成功即刷新账户名显示 */
      void api.syncStatus().then((st) => { if (st) setAccount(st.account); });
      setPassword('');
      showToast(result?.message ?? '已保存，正在后台同步…');
      /* 首连且本地有未绑定的直连源 → 弹「同步本地订阅到后端」
         （不自动推：推送会改变服务端数据，必须用户确认） */
      if (result?.firstConnect && result.unboundLocalFeeds > 0) {
        setPendingLocalSync(result.unboundLocalFeeds);
      }
      /* 全后台链：feeds 阶段（快）→ states 阶段（慢，含全量对账）→ 直连抓新源 */
      void api
        .syncPhase('feeds')
        .then(async () => {
          await reloadFromBackend();
          showToast('已拉取订阅源，正在同步文章状态…');
          return api.syncPhase('states', true);
        })
        .then(async () => {
          await reloadFromBackend();
          return api.refreshAllFeeds().catch(() => null);
        })
        .then(() => reloadFromBackend())
        .then(() => {
          useAppStore.setState({ syncStatus: 'synced', syncConnected: true });
          showToast('后端同步完成');
        })
        .catch((e: unknown) => {
          const m = extractError(e);
          showToast(`后台同步失败：${m}`, { label: '重试', run: () => { void doSaveAndSync(); } });
        });
    } catch (e) {
      showToast(`保存失败：${endpointHint(extractError(e))}`);
    } finally {
      setSaving(false);
    }
  };

  /** 把本地直连订阅推送到后端（首连弹窗确认与手动按钮共用）。
   * 幂等：已绑定的跳过、服务端已有同 URL（409）回查绑定不报错 */
  const doSyncLocal = async () => {
    if (dataMode !== 'tauri') {
      showToast('演示模式不支持同步');
      return;
    }
    setSyncingLocal(true);
    try {
      const msg = await api.syncLocalFeeds();
      await reloadFromBackend();
      setPendingLocalSync(0);
      showToast(msg ?? '同步完成');
    } catch (e) {
      showToast(`同步本地订阅失败：${extractError(e)}`);
    } finally {
      setSyncingLocal(false);
    }
  };

  const doDisconnect = async () => {
    setSaving(true);
    try {
      const msg = await api.syncDisconnect();
      setConnected(false);
      setAccount(null);
      setPassword('');
      await reloadFromBackend();
      showToast(msg ?? '已断开连接');
    } catch (e) {
      showToast(`断开失败：${extractError(e)}`);
    } finally {
      setSaving(false);
    }
  };

  if (dataMode !== 'tauri') {
    return (
      <>
        <div className="settings-group-title">后端配置</div>
        <SettingCard title="演示模式" desc="同步功能需要运行在 Tauri 客户端内（npm run tauri dev）">
          <span className="about-arch-tag">不支持同步</span>
        </SettingCard>
      </>
    );
  }

  return (
    <>
      <div className="settings-group-title">后端配置</div>
      <SettingCard
        title="连接状态"
        desc={connected
          ? `${account ? `账户 ${account} · ` : ''}上次同步 ${lastSync > 0 ? new Date(lastSync * 1000).toLocaleString() : '从未'}`
          : '未连接（客户端可独立使用：直连抓取、阅读、收藏均正常）'}
      >
        <span className="about-arch-tag">{connected ? (account ?? '已连接') : '未连接'}</span>
      </SettingCard>
      <SettingCard
        title="同步协议"
        desc="两种协议共用 Miniflux「集成」凭据，切换不丢数据。"
      >
        {/* REQ-008：全应用统一控件——此前是全仓唯一的原生 <select>，
            深浅主题外观与展开行为都与 FluxDropdown 不一致 */}
        <FluxDropdown
          width={220}
          value={protocol}
          onChange={(v) => setProtocol(v === 'fever' ? 'fever' : 'greader')}
          options={[
            { value: 'greader', label: 'Google Reader（推荐）' },
            { value: 'fever', label: 'Fever' },
          ]}
        />
      </SettingCard>
      <SettingCard title="后端 Endpoint" desc={ENDPOINT_DESC}>
        <input
          type="text"
          className="setting-input"
          placeholder={ENDPOINT_PLACEHOLDER}
          value={endpoint}
          onChange={(e) => setEndpoint(e.target.value)}
        />
      </SettingCard>
      <SettingCard
        title="用户名"
        desc="Miniflux「集成」页单独配置的用户名（Google Reader / Fever 共用，非 Miniflux 账号密码）"
      >
        <input
          type="text"
          className="setting-input"
          placeholder="集成用户名"
          value={username}
          onChange={(e) => setUsername(e.target.value)}
        />
      </SettingCard>
      <SettingCard
        title="密码"
        desc={connected ? '已保存（出于安全不回显）。留空提交 = 保持当前密码；填写新值 = 更换账号' : '集成密码（Google Reader / Fever 共用）'}
      >
        <input
          type="password"
          className="setting-input"
          placeholder={connected ? '●●●●●●●●（已保存）' : '密码'}
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
      </SettingCard>
      <div className="settings-action-row">
        <button className="toggle-action-btn" disabled={testing || saving} onClick={() => void doTest()}>
          {testing ? '测试中…' : '测试连接'}
        </button>
        <button className="toggle-action-btn btn-primary" disabled={testing || saving} onClick={() => void doSaveAndSync()}>
          {saving ? '保存中…' : '保存并同步'}
        </button>
        {connected && (
          <button
            className="toggle-action-btn"
            disabled={syncingLocal || saving}
            onClick={() => void doSyncLocal()}
            title="把本地直连添加的订阅推送到服务端（已同步的自动跳过）"
          >
            {syncingLocal ? '同步中…' : '同步本地订阅'}
          </button>
        )}
        {connected && (
          <button className="toggle-action-btn" disabled={testing || saving} onClick={() => setConfirmDisconnect(true)}>
            断开连接
          </button>
        )}
      </div>
      <div className="mini-dialog-hint" style={{ marginTop: 8 }}>
        「测试连接」只验证连通性（秒级）；「保存并同步」会立即在后台拉取订阅与文章状态。
        已读/收藏等变更约 1 秒内推送到服务端；断开连接会移除服务端拉取的订阅与文章。
      </div>

      <div className="settings-group-title" style={{ marginTop: 20 }}>自动同步</div>
      <SettingCard
        title="同步模式"
        desc="本机抓取 = 直连各订阅源（离线可用）；跟随服务端 = 内容由后端提供，多设备一致"
      >
        <FluxDropdown
          width={130}
          value={settings.syncMode}
          onChange={(v) => updateSettings({ syncMode: v as 'direct' | 'hybrid' })}
          options={[
            { value: 'direct', label: '本机抓取' },
            { value: 'hybrid', label: '跟随服务端' },
          ]}
        />
      </SettingCard>
      <SettingCard title="后台自动同步" desc="按刷新间隔到期时自动做轻量增量同步（拉取服务端状态变化）">
        <Switch checked={settings.autoSync} onChange={(v) => updateSettings({ autoSync: v })} />
      </SettingCard>

      <CacheCleanupSection />
      <ConfigSyncSection />

      <ConfirmDialog
        open={confirmDisconnect}
        title="断开后端连接"
        message="断开后将移除从服务端拉取的订阅与文章（含已读/收藏绑定），本地直连添加的订阅不受影响。确定断开吗？"
        confirmText="断开并清理"
        onConfirm={() => { setConfirmDisconnect(false); void doDisconnect(); }}
        onCancel={() => setConfirmDisconnect(false)}
      />

      {/* 首连且本地有未绑定源：询问是否把本地订阅推送到服务端
          （推送会改变服务端数据——必须用户确认，不自动执行） */}
      <ConfirmDialog
        open={pendingLocalSync > 0}
        title="同步本地订阅到后端"
        message={`检测到本地有 ${pendingLocalSync} 个直连添加的订阅尚未同步到服务端。是否现在同步？同步后它们会出现在你的后端账户中，其他设备也能看到。`}
        confirmText="同步到服务端"
        onConfirm={() => { setPendingLocalSync(0); void doSyncLocal(); }}
        onCancel={() => setPendingLocalSync(0)}
      />
    </>
  );
}
