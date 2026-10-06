import { useEffect, useState } from 'react';
import { useAppStore } from '../../store';
import { api, extractError, type SyncQueueStats } from '../../lib/api';
import { syncStateSummary } from '../../lib/syncPill';
import { FluxDropdown, Switch, SettingCard, ConfirmDialog } from '../primitives';
import { CacheCleanupSection } from './CacheCleanupSection';
import { ConfigSyncSection } from './ConfigSyncSection';
import { ENDPOINT_DESC, ENDPOINT_PLACEHOLDER, endpointHint } from './endpointHint';
import { syncFailureMessage } from '../../store/syncErrors';

/* ---------- TAB 6: 同步 ---------- */

/** TASK-116：拉取同步队列统计进摘要卡。失败静默——纯提示性数据（TASK-067 N10
 *  口径）。模块级函数（只依赖注入的 setter），供挂载 effect 与同步链尾复用。 */
function refreshQueueStats(setter: (q: SyncQueueStats | null) => void) {
  void api.syncQueueStats()
    .then((q) => { if (q) setter(q); })
    .catch(() => { /* 统计拉取失败静默兜底（默认零值可用） */ });
}

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
  /** TASK-116 X2：同步队列统计（等待/部分失败 + 最新错误摘要） */
  const [queueStats, setQueueStats] = useState<SyncQueueStats | null>(null);
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
    }).catch(() => { /* TASK-067 N10：挂载期状态拉取失败静默兜底（默认态可用） */ });
    refreshQueueStats(setQueueStats);
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
      /* 全后台链：feeds 阶段（快）→ states 阶段（慢，含全量对账）→ 直连抓新源。
         TASK-058：后端在 errors 非空时仍返回 Ok（单项失败不中断整链），故失败必须
         在此**主动读取** report 才能被用户看到。成功路径的既有文案与顺序逐字不变。 */
      const failures: string[] = [];
      /* TASK-100 P3-9：后端对账删除计数（SyncReport.removed_feeds）——纯信息展示 */
      let removedFeeds = 0;
      void api
        .syncPhase('feeds')
        .then(async (feedsReport) => {
          await reloadFromBackend();
          const fail = syncFailureMessage(feedsReport);
          if (fail) failures.push(fail);
          else showToast('已拉取订阅源，正在同步文章状态…');
          if (feedsReport?.removed_feeds) removedFeeds += feedsReport.removed_feeds;
          return api.syncPhase('states', true);
        })
        .then(async (statesReport) => {
          await reloadFromBackend();
          const fail = syncFailureMessage(statesReport);
          if (fail) failures.push(fail);
          if (statesReport?.removed_feeds) removedFeeds += statesReport.removed_feeds;
          return api.refreshAllFeeds().catch(() => null);
        })
        .then(() => reloadFromBackend())
        .then(() => {
          useAppStore.setState({ syncStatus: 'synced', syncConnected: true });
          /* 有失败项时给出「有 N 项失败」而不是纯粹的「后端同步完成」；
             errors 为空则与改动前**逐字相同**。 */
          showToast(failures.length > 0 ? failures.join('；') : '后端同步完成');
          /* TASK-100 P3-9：对账移除了远端已删除的订阅源——纯信息展示，单列一条 */
          if (removedFeeds > 0) showToast(`本次对账移除 ${removedFeeds} 个已在服务端删除的订阅源`);
          /* TASK-116：本次同步链可能已清空队列或标记失败项，摘要卡跟着刷新 */
          refreshQueueStats(setQueueStats);
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
      /* TASK-100 P3-8：断开与「断开后的本地刷新」分开处理——此前 reload 失败
         重抛被同一个 catch 捕获，把「已断开成功」误报成「断开失败」。 */
      try {
        await reloadFromBackend();
      } catch (reloadErr) {
        showToast(`已断开连接，但本地刷新失败：${extractError(reloadErr)}`);
        return;
      }
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

  /* TASK-102 X3：账户前缀提为常量——空串字面量与「从未」引号留在同一行时，
     文案扫描会把两个引号间的模板内容误配成假长文案（'…' 对跨模板配对）；
     分行后字面量配对只在单行内成立，扫描恢复零误报。渲染输出不变。 */
  const accountTag = account ? `账户 ${account} · ` : '';

  /* TASK-116 X2：四态摘要口径收口到纯函数 syncStateSummary（src/lib/syncPill.ts，
     契约 X2/X3 如实原则；真值表由回归网 t116 断言驱动）。 */
  const waiting = queueStats?.waiting ?? 0;
  const failedCount = queueStats?.failed ?? 0;
  const syncStateDesc = syncStateSummary(
    queueStats ?? { waiting: 0, failed: 0, last_error: null },
    lastSync,
  );

  return (
    <>
      <div className="settings-group-title">后端配置</div>
      <SettingCard
        title="连接状态"
        desc={connected
          ? `${accountTag}上次同步 ${lastSync > 0 ? new Date(lastSync * 1000).toLocaleString() : '从未'}`
          : '未连接（客户端可独立使用：直连抓取、阅读、收藏均正常）'}
      >
        <span className="about-arch-tag">{connected ? (account ?? '已连接') : '未连接'}</span>
      </SettingCard>
      {/* TASK-116 X2：四态摘要卡。状态标签 = 四态中的当下态（部分失败 > 等待同步 >
         本地已保存）——「远端已确认」随成功 prune 即时出队、不可作常驻态展示（X3）；
         desc 收尾即「本地已保存」说明句（状态变更已事务化落库，TASK-108） */}
      <SettingCard title="同步状态" desc={syncStateDesc}>
        <span className="about-arch-tag">
          {failedCount > 0 ? '部分失败' : waiting > 0 ? '等待同步' : '本地已保存'}
        </span>
      </SettingCard>
      {/* REQ-008：全应用统一控件——此前是全仓唯一的原生 <select>，
          深浅主题外观与展开行为都与 FluxDropdown 不一致。
          TASK-102：FluxDropdown 必须是 SettingCard 的直接子元素（.setting-card
          双列布局的控件列贴右缘）——包进裸 div 会被提示文本撑宽、把下拉挤离
          右缘（X1 错位根因）；API 密码提示改放 desc 位（≤32 字），不设常驻 hint。 */}
      <SettingCard
        title="同步协议"
        desc="Fever / GReader 均用「API 密码」，非登录密码"
      >
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
      <SettingCard
        title="后端 Endpoint"
        desc={ENDPOINT_DESC}
      >
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
        desc="Miniflux「集成」页配置的用户名，GReader / Fever 共用（非账号密码）"
      >
        <input
          type="text"
          className="setting-input"
          placeholder="集成用户名"
          value={username}
          onChange={(e) => setUsername(e.target.value)}
        />
      </SettingCard>
      {/* TASK-102：断开态不设 desc——「集成密码」字面解释复述标题，共用语义已由
          协议卡 desc 与用户名卡承载（X3：无复述标题的 desc） */}
      <SettingCard
        title="密码"
        desc={connected ? '已保存（出于安全不回显）。留空提交 = 保持当前密码；填写新值 = 更换账号' : undefined}
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
      {/* TASK-102：单行 ≤40 字，只保留两按钮语义对齐——「断开会移除拉取内容」由
          断开确认框承载、「已读/收藏约 1 秒回传」为解释性冗余，均不再常驻 */}
      <div className="mini-dialog-hint" style={{ marginTop: 8 }}>
        「测试连接」仅验证登录；「保存并同步」确认后拉取订阅与文章。
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
