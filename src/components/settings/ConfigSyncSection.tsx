import { useEffect, useState } from 'react';
import { awaitAiWriteFrontier, awaitAppSettingsWriteFrontier, confirmAppliedConfig, markAiConfigUnconfirmed, markAppSettingsUnconfirmed, readBackAppliedAppSettings, runAppSettingsApplyBoundary, runConfigApplyBarrier, useAppStore } from '../../store';
import { api, extractError, extractErrorCode } from '../../lib/api';
import type { ConfigSyncPlanItem, ConfigSyncPlanResult } from '../../lib/api';
import { openExternal } from '../../lib/external';
import { FluxDropdown, SettingCard } from '../primitives';

/* ---------- 配置同步（GitHub Gist / WebDAV）---------- */

type ApplyMode = 'merge' | 'mirror';

/** 预览条目计数：源/分类分开（展示与文案都按这个口径）。 */
function countPlanItems(items: ConfigSyncPlanItem[]): { feeds: number; folders: number } {
  let feeds = 0;
  let folders = 0;
  for (const it of items) {
    if (it.kind === 'feed') feeds += 1;
    else folders += 1;
  }
  return { feeds, folders };
}

/** 预览里的关键对象描述（删除清单用；含数量）。 */
function planItemLabel(it: ConfigSyncPlanItem): string {
  if (it.kind === 'feed') {
    return it.affectedArticleCount !== undefined ? `${it.title}（${it.affectedArticleCount} 篇文章）` : it.title;
  }
  return it.feedCount !== undefined ? `${it.name}（${it.feedCount} 个源）` : it.name;
}

export function ConfigSyncSection() {
  const showToast = useAppStore((s) => s.showToast);
  const reloadFromBackend = useAppStore((s) => s.reloadFromBackend);
  const [backend, setBackend] = useState<'gist' | 'webdav'>('gist');
  const [mode, setMode] = useState<ApplyMode>('merge');
  const [token, setToken] = useState('');
  const [server, setServer] = useState('');
  const [username, setUsername] = useState('');
  const [status, setStatus] = useState<{ configured: boolean; backend?: string; lastUpload?: string } | null>(null);
  const [busy, setBusy] = useState(false);
  /** OPT-010：下载预览的待确认结果（payload 原文 + 结构化 plan）。
   * 取消 = 直接清除（零写入）；apply 失败/签名失效也必须清除（要求重新预览）。 */
  const [preview, setPreview] = useState<{ payload: string; plan: ConfigSyncPlanResult } | null>(null);

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
    }).catch(() => { /* TASK-067 N10：挂载期状态拉取失败静默兜底 */ });
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
      /* OPT-011 R5：上传是**读取已保存配置**的快照——先等发起时已存在的 AI 写
         前沿落定（快照不漏用户已点写的 AI 值）。不设写门：上传的网络往返不
         阻塞之后任何 AI 操作（之后的点击乐观显示、请求照常发出）。
         R3：app_settings（界面设置）同一语义——发起时前沿等待，快照不漏已点写，
         不设门。 */
      await awaitAiWriteFrontier();
      await awaitAppSettingsWriteFrontier();
      const at = await api.configSyncUpload();
      setStatus({ configured: true, backend, lastUpload: at });
      showToast('配置已上传');
    } catch (e) {
      showToast(`上传失败：${extractError(e)}`);
    } finally {
      setBusy(false);
    }
  };

  /** 第一步：下载 + 只读预览（结构化变更清单 + 两个签名）。不写任何本地数据。 */
  const doPreview = async () => {
    setBusy(true);
    try {
      const payload = await api.configSyncDownload();
      const plan = await api.configSyncPlan(payload, mode);
      if (!plan) {
        showToast('预览失败：当前环境不支持配置同步');
        return;
      }
      setPreview({ payload, plan });
    } catch (e) {
      showToast(`下载失败：${extractError(e)}`);
    } finally {
      setBusy(false);
    }
  };

  /** 第二步：用户确认后应用（携带预览签名做 CAS 复核）。 */
  const doApply = async () => {
    if (!preview) return;
    setBusy(true);
    const { payload, plan } = preview;
    try {
      /* OPT-011 R5：确认后进入受控写屏障——同步捕获并等待「进入时已存在的
         AI 写前沿」，应用期间的 AI 点击乐观可见、请求排在应用之后；屏障不
         等待后来写（无自锁），失败/取消都在 finally 释放、错误如实提示。
         R3：app_settings 同一约定（嵌套边界：AI 门 + 设置门同时生效）——
         应用期间的新点击乐观可见、请求排在应用与回读之后；应用+回读的
         app_settings 真实回读在同一窗口内完成，键经回读保留、请求载荷在
         发送时重建（不清空点击、不把远端覆盖打回）。 */
      let readbackError: string | null = null;
      let settingsReadbackError: string | null = null;
      const r = await runConfigApplyBarrier(() => runAppSettingsApplyBoundary(async () => {
        const res = await api.configSyncApply(payload, plan.mode, plan.snapshotHash, plan.currentVersion);
        try {
          /* 真实配置回读确认/对齐——不把可能被查询代际淘汰的 reload 当已确认。 */
          await confirmAppliedConfig();
        } catch (e) {
          /* 应用已整包覆写后端 AI 配置而本地没有读回实际值：受影响键标记
             未确认（后续失败不得声称「已恢复原值」），如实告知。 */
          readbackError = extractError(e);
          markAiConfigUnconfirmed();
        }
        try {
          /* R3：app_settings 真实回读（应用窗口内、门未释放）——刷新可视设置与
             确认基线；窗口内新点击的键保留本地意图值。 */
          await readBackAppliedAppSettings();
        } catch (e) {
          /* 应用已提交而界面设置没有读回：确认基线置不可信（后续失败不假称
             已恢复），如实告知。 */
          settingsReadbackError = extractError(e);
          markAppSettingsUnconfirmed();
        }
        return res;
      }));
      /* 应用结果落地后的全量刷新（entries/计数）；它不是确认依据（确认由上面
         的真实回读完成），失败由刷新路径自身提示。 */
      void reloadFromBackend().catch(() => null);
      /* 应用成功后预览消费完毕：清除，下一次必须重新下载预览。 */
      setPreview(null);
      // TASK-074：如实区分「新增 / 已更新 / 已跳过」——此前把已更新数当成
      // 「跳过 M 个已存在」展示，数字含义是错的。
      // OPT-010：镜像删除数如实回报（合并模式恒为 0）。
      const parts = [`新增 ${r.imported} 个源`];
      if (r.updated > 0) parts.push(`更新 ${r.updated} 个`);
      if (r.skipped > 0) parts.push(`跳过 ${r.skipped} 个（内容一致）`);
      const removedFeeds = r.removedFeeds ?? 0;
      const removedFolders = r.removedFolders ?? 0;
      if (removedFeeds > 0 || removedFolders > 0) {
        parts.push(`删除 ${removedFeeds} 个源、${removedFolders} 个分类（含 ${r.affectedArticleCount ?? 0} 篇文章）`);
      }
      showToast(`配置已应用：${parts.join('，')}`);
      if (readbackError) {
        showToast(`配置已应用，但配置回读失败：${readbackError}（本地界面值未确认）`);
      }
      if (settingsReadbackError) {
        showToast(`配置已应用，但界面设置回读失败：${settingsReadbackError}（本地界面值未确认）`);
      }
      // OPT-006：导入不会改动活动连接；若含连接信息则提示到「后端配置」激活
      if (r.pendingConnection) {
        showToast('检测到连接信息：请到「后端配置」重新输入密码后保存以激活（当前连接不受影响）');
      }
    } catch (e) {
      /* 失败（含 stalePreview 签名失效）：预览一律作废，要求重新下载预览——
         绝不对旧预览重新计算后直接执行。P3-2：应用失败的提示不写「下载失败」
         （失败发生在应用阶段，误导用户以为问题在过去的一步）。 */
      setPreview(null);
      showToast(`应用失败：${extractError(e)}`);
      if (extractErrorCode(e) === 'stalePreview') {
        showToast('预览已失效：本地配置在预览后已变化，请重新下载预览确认后再应用');
      }
    } finally {
      setBusy(false);
    }
  };

  const plan = preview?.plan ?? null;
  const addCounts = plan ? countPlanItems(plan.additions) : null;
  const updCounts = plan ? countPlanItems(plan.updates) : null;
  const remCounts = plan ? countPlanItems(plan.removals) : null;

  return (
    <>
      <div className="settings-group-title" style={{ marginTop: 20 }}>GitHub 同步</div>
      {ghAccount ? (
        <SettingCard
          title="已登录 GitHub"
          desc={`账户 ${ghAccount.login} · 配置经私有 Gist 同步（token 仅存本机）`}
        >
          <button className="toggle-action-btn btn-danger-text" disabled={ghLoggingIn} onClick={() => void doGhDisconnect()}>
            断开
          </button>
        </SettingCard>
      ) : ghFlow ? (
        <SettingCard
          title="等待授权"
          desc="在浏览器授权页输入下方代码完成授权，成功后此卡自动消失"
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
          : '同步订阅与分类结构、AI 开关、界面设置（不含正文、AI 服务配置与缓存）'}
      >
        <span className="about-arch-tag">{status?.configured ? '已配置' : '未配置'}</span>
      </SettingCard>
      {/* TASK-102：三处 desc 压缩至 ≤48 字，约束语义（classic PAT / gist scope /
          fine-grained 不支持 / 配置文件名）逐条保留 */}
      <SettingCard title="同步后端" desc="Gist 可网页登录自动配置，或手填 classic PAT；WebDAV 填服务器与账号">
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
      {/* OPT-010：应用模式（默认合并；镜像必须经预览 + 明确确认）。 */}
      <SettingCard
        title="应用模式"
        desc={mode === 'mirror'
          ? '镜像：以远端为准，确认后删除本地多出的源与分类'
          : '合并：保留本地独有的源与分类，只更新远端列出的对象'}
      >
        <FluxDropdown
          width={140}
          value={mode}
          onChange={(v) => {
            setMode(v as ApplyMode);
            /* 换模式 = 换预览语义：旧预览立即作废（不得跨模式确认）。 */
            setPreview(null);
          }}
          options={[
            { value: 'merge', label: '合并' },
            { value: 'mirror', label: '镜像' },
          ]}
        />
      </SettingCard>
      {backend === 'gist' ? (
        <SettingCard title="GitHub Token（classic PAT）" desc="手动填入；classic PAT 需勾 gist scope，fine-grained 不支持">
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
          {/* TASK-102：示例 URL 由输入框 placeholder 表达，desc 只保留配置文件名（约束语义） */}
          <SettingCard title="WebDAV 服务器" desc="配置在服务器存为 fluxreader-config.json">
            <input
              type="text"
              className="setting-input"
              placeholder="https://dav.example.com/path"
              value={server}
              onChange={(e) => setServer(e.target.value)}
            />
          </SettingCard>
          <SettingCard title="WebDAV 用户名">
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
      {/* OPT-010 预览卡：下载后只读展示将发生的新增/更新/删除，用户确认才应用；
          取消 = 清除预览（零写入）；失败/签名失效也清除并要求重新预览。 */}
      {plan && (
        <div className="setting-card config-plan-preview">
          <div className="setting-card-text">
            <h5>{plan.mode === 'mirror' ? '镜像预览（以远端为准）' : '合并预览（保留本地独有）'}</h5>
            <p>
              {plan.mode === 'mirror'
                ? '确认后将删除本地多出的源与分类：仅影响本机，不会退订服务端订阅。'
                : '只更新远端列出的对象；本地独有的源与分类不会被删除。'}
            </p>
          </div>
          <div className="config-plan-body">
            <div className="config-plan-line">新增：源 {addCounts?.feeds ?? 0} 个 / 分类 {addCounts?.folders ?? 0} 个</div>
            <div className="config-plan-line">更新：源 {updCounts?.feeds ?? 0} 个 / 分类 {updCounts?.folders ?? 0} 个</div>
            {plan.mode === 'mirror' && (
              <div className="config-plan-line config-plan-danger">
                删除：源 {remCounts?.feeds ?? 0} 个 / 分类 {remCounts?.folders ?? 0} 个（含 {plan.affectedArticleCount} 篇文章与读藏状态，随本地级联删除）
              </div>
            )}
            {plan.removals.length > 0 && (
              <div className="config-plan-line">
                将删除：{plan.removals.slice(0, 5).map((it, i) => `${i > 0 ? '、' : ''}${planItemLabel(it)}`).join('')}
                {plan.removals.length > 5 ? ` 等 ${plan.removals.length} 项` : ''}
              </div>
            )}
            <div className="config-plan-line">
              界面设置：覆盖 {plan.settingsChangedKeys.length} 个键 / 删除 {plan.settingsRemovedKeys.length} 个键；AI 服务配置（模型/密钥）不参与同步。
            </div>
            {plan.pendingConnection && (
              <div className="config-plan-line">快照含连接信息建议：应用后需到「后端配置」重新输入密码激活（当前连接不受影响）。</div>
            )}
            {plan.warnings.map((w, i) => (
              <div key={i} className="config-plan-line config-plan-warning">{w}</div>
            ))}
          </div>
          <div className="mini-dialog-actions">
            <button
              className="toggle-action-btn"
              disabled={busy}
              onClick={() => setPreview(null)}
            >
              取消
            </button>
            <button
              className={plan.mode === 'mirror' ? 'toggle-action-btn btn-danger' : 'toggle-action-btn btn-primary'}
              disabled={busy}
              onClick={() => void doApply()}
            >
              {busy ? '处理中…' : plan.mode === 'mirror' ? '确认镜像应用' : '确认应用'}
            </button>
          </div>
        </div>
      )}
      <div className="settings-action-row">
        <button className="toggle-action-btn" disabled={busy} onClick={() => void doSave()}>
          保存凭据
        </button>
        <button className="toggle-action-btn btn-primary" disabled={busy || !status?.configured} onClick={() => void doUpload()}>
          {busy ? '处理中…' : '上传配置'}
        </button>
        <button className="toggle-action-btn" disabled={busy || !status?.configured} onClick={() => void doPreview()}>
          下载预览
        </button>
      </div>
      {/* TASK-102：压缩至 ≤48 字单句——覆盖语义（约束）保留，
          「多设备换机先上传新机下载」为操作指引花絮 → 删除 */}
      <div className="mini-dialog-hint" style={{ marginTop: 8 }}>
        下载先预览：合并保留本地独有；镜像删除本地多出对象，确认后才写入。
      </div>
    </>
  );
}
