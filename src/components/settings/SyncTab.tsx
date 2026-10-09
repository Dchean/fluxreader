import { useEffect, useState } from 'react';
import { useAppStore } from '../../store';
import { api, extractError, type SyncQueueStats, type PendingConnection } from '../../lib/api';
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
  /** OPT-006：配置导入留下的待确认连接建议（需重新输入凭据后保存才会激活） */
  const [pendingConn, setPendingConn] = useState<PendingConnection | null>(null);
  /** 激活意图：用户点了「填入表单并激活」后记录建议版本，保存时回传后端做 CAS。
   * 普通保存（未点激活）= undefined，后端不消费任何建议。 */
  const [activateVersion, setActivateVersion] = useState<number | undefined>(undefined);

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
    /* OPT-006：待确认连接建议（配置导入只落建议，激活要走保存流程） */
    void api.syncPendingConnection().then((p) => setPendingConn(p)).catch(() => { /* 同上：静默兜底 */ });
    refreshQueueStats(setQueueStats);
  }, [dataMode]);

  /* TASK-124：摘要卡实时跟随 sync-queue-changed——store 三字段由 App.tsx 监听器
     （事件）与 reloadFromBackend（顺带刷新）写入（单一状态源）。这里订阅 store，
     变化镜像进本地 queueStats（挂载/保存链尾的 refreshQueueStats 主动拉取保留：
     打开设置页即取一次最新真值；事件缺失时摘要卡维持 t116 既有行为）。
     setState 位于订阅回调（事件驱动的外部系统同步），非 effect 体同步 setState。 */
  useEffect(
    () =>
      useAppStore.subscribe((s) => {
        setQueueStats({ waiting: s.syncWaiting, failed: s.syncFailed, last_error: s.syncQueueLastError });
      }),
    [],
  );

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

  /** 保存并同步：保存（轻量测试+事务落库）→ 后台链路（feeds → states → 直连抓新源）
      全部在 `saving` 覆盖下 await 完成——F06 修复点之一：此前后台链 fire-and-forget，
      「保存中…」在真正同步结束前就消失，失败也可能只留一个转瞬即逝的 toast。
      已连接且密码留空 = 复用已存密码（仅改 Endpoint 的场景，同账号更新保留数据） */
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
      const result = await api.syncSave(protocol, endpoint.trim(), username.trim(), password.trim(), activateVersion);
      setConnected(true);
      /* OPT-006 R1：激活成功由后端 CAS 消费建议；普通保存不消费。统一重读一次
         真实状态（激活成功 = 无建议；普通保存 = 建议仍在，继续展示）。 */
      setActivateVersion(undefined);
      void api.syncPendingConnection().then((p) => setPendingConn(p)).catch(() => { /* 静默兜底 */ });
      /* 保存成功即刷新账户名显示 */
      void api.syncStatus().then((st) => { if (st) setAccount(st.account); });
      setPassword('');
      showToast(result?.message ?? '已保存，正在后台同步…');
      /* 首连且本地有未绑定的直连源 → 弹「同步本地订阅到后端」
         （不自动推：推送会改变服务端数据，必须用户确认） */
      if (result?.firstConnect && result.unboundLocalFeeds > 0) {
        setPendingLocalSync(result.unboundLocalFeeds);
      }
      /* TASK-058：后端在 errors 非空时仍返回 Ok（单项失败不中断整链），故失败必须
         在链路内**主动读取** report 才能被用户看到。成功路径的既有文案与顺序不变。 */
      const failures: string[] = [];
      /* TASK-100 P3-9：后端对账删除计数（SyncReport.removed_feeds）——纯信息展示 */
      let removedFeeds = 0;
      try {
        const feedsReport = await api.syncPhase('feeds');
        await reloadFromBackend();
        const fail = syncFailureMessage(feedsReport);
        if (fail) failures.push(fail);
        else showToast('已拉取订阅源，正在同步文章状态…');
        if (feedsReport?.removed_feeds) removedFeeds += feedsReport.removed_feeds;
        const statesReport = await api.syncPhase('states', true);
        await reloadFromBackend();
        const fail2 = syncFailureMessage(statesReport);
        if (fail2) failures.push(fail2);
        if (statesReport?.removed_feeds) removedFeeds += statesReport.removed_feeds;
        await api.refreshAllFeeds().catch(() => null);
        await reloadFromBackend();
      } catch (e) {
        /* 链路任一步硬失败：必须显式呈现错误（不假成功），并给出重试入口 */
        showToast(`后台同步失败：${extractError(e)}`, { label: '重试', run: () => { void doSaveAndSync(); } });
        return;
      }
      useAppStore.setState({ syncStatus: 'synced', syncConnected: true });
      /* 有失败项时给出「有 N 项失败」而不是纯粹的「后端同步完成」；
         errors 为空则与改动前**逐字相同**。 */
      showToast(failures.length > 0 ? failures.join('；') : '后端同步完成');
      /* TASK-100 P3-9：对账移除了远端已删除的订阅源——纯信息展示，单列一条 */
      if (removedFeeds > 0) showToast(`本次对账移除 ${removedFeeds} 个已在服务端删除的订阅源`);
      /* TASK-116：本次同步链可能已清空队列或标记失败项，摘要卡跟着刷新 */
      refreshQueueStats(setQueueStats);
    } catch (e) {
      showToast(`保存失败：${endpointHint(extractError(e))}`);
      /* OPT-006 R1：保存失败（含 staleActivation：建议在验证期间被新导入替换）
         之后重读建议——界面必须显示当前真实待确认状态，不能残留旧版本。 */
      void api.syncPendingConnection().then((p) => { setPendingConn(p); setActivateVersion(undefined); }).catch(() => { /* 静默兜底 */ });
    } finally {
      setSaving(false);
    }
  };

  /** OPT-006：把配置导入的待确认建议填入表单（不激活——必须重新输入密码后保存） */
  const doApplyPending = () => {
    if (!pendingConn) return;
    if (pendingConn.sync_protocol === 'fever' || pendingConn.sync_protocol === 'greader') {
      setProtocol(pendingConn.sync_protocol);
    }
    if (pendingConn.greader_endpoint) setEndpoint(pendingConn.greader_endpoint);
    if (pendingConn.greader_username) setUsername(pendingConn.greader_username);
    setPassword('');
    /* 记录激活版本：保存时回传后端（后端独立做版本/身份 CAS，不信任 UI） */
    setActivateVersion(pendingConn.version);
    showToast('已填入待确认连接信息：请重新输入密码后点「保存并同步」激活');
  };

  /** OPT-006：放弃待确认建议（不触碰当前连接与本地数据） */
  const doDismissPending = async () => {
    try {
      await api.syncDismissPendingConnection();
      setPendingConn(null);
      setActivateVersion(undefined);
      showToast('已忽略配置导入的连接建议');
    } catch (e) {
      showToast(`忽略失败：${extractError(e)}`);
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
      {/* OPT-006：配置导入的待确认连接建议——只展示非敏感字段；激活必须重新输入
          密码后经「保存并同步」专用流程（历史实现的导入直接改写活动地址 + 旧密码，
          会把旧密码拼给新服务端）。 */}
      {pendingConn && (
        <SettingCard
          title="配置导入的连接建议"
          desc={`来自配置同步：${pendingConn.sync_protocol === 'fever' ? 'Fever' : 'GReader'} · ${pendingConn.greader_endpoint ?? '（未提供地址）'} · ${pendingConn.greader_username ?? '（未提供用户名）'}。激活需重新输入密码后保存，当前连接不受影响。`}
        >
          <div className="settings-action-row">
            <button className="toggle-action-btn btn-primary" disabled={saving} onClick={doApplyPending}>
              填入表单并激活
            </button>
            <button className="toggle-action-btn" disabled={saving} onClick={() => void doDismissPending()}>
              忽略
            </button>
          </div>
        </SettingCard>
      )}
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
