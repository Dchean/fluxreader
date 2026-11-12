import { useEffect, useRef, useState } from 'react';
import { useAppStore } from '../../store';
import type { LocalSyncMode } from '../../lib/api';
import { FluxDropdown, SettingCard } from '../primitives';

const syncTime = (seconds: number) => seconds > 0 ? new Date(seconds * 1000).toLocaleString() : '尚未确认';

/** 连接事实在 store；编辑草稿只在本表单。状态事件不能取得已编辑字段的所有权。 */
export function LocalSyncSection() {
  const status = useAppStore((s) => s.localSyncState);
  const error = useAppStore((s) => s.localSyncError);
  const busy = useAppStore((s) => s.localSyncBusy);
  const rssConnected = useAppStore((s) => s.syncConnected);
  const restoreBusy = useAppStore((s) => s.libraryRestoreBusy);
  const settingsOpen = useAppStore((s) => s.settingsOpen);
  const refresh = useAppStore((s) => s.refreshLocalSyncStatus);
  const [endpoint, setEndpoint] = useState('');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [mode, setMode] = useState<LocalSyncMode>('scheduled');
  const [interval, setInterval] = useState('15');
  const [validation, setValidation] = useState<string | null>(null);
  const edited = useRef(false);
  const revision = useRef(0);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => { void refresh().catch(() => {}); }, [refresh, settingsOpen]);
  useEffect(() => {
    if (!status || edited.current) return;
    setEndpoint(status.endpoint ?? '');
    setUsername(status.username ?? '');
    setMode(status.mode);
    setInterval(String(status.interval_minutes));
  }, [status]);
  const touch = () => { edited.current = true; revision.current++; setValidation(null); };
  const connect = async () => {
    const intervalMinutes = Number(interval);
    if (!endpoint.trim() || !username.trim() || !password) {
      setValidation('请填写 WebDAV 地址、用户名和密码'); return;
    }
    if (!Number.isInteger(intervalMinutes) || intervalMinutes < 1 || intervalMinutes > 1440) {
      setValidation('同步间隔须为 1–1440 分钟的整数'); return;
    }
    const version = revision.current;
    const ok = await useAppStore.getState().connectLocalSync({ endpoint: endpoint.trim(), username: username.trim(), password, mode, intervalMinutes });
    if (ok && mounted.current && revision.current === version) {
      setPassword(''); edited.current = false;
    }
  };
  const disabled = !!busy || restoreBusy;
  return (
    <>
      <div className="settings-group-title" style={{ marginTop: 20 }}>本地资料库 · WebDAV</div>
      <SettingCard title="持续同步" desc="同步订阅、分类、普通设置、读藏及未读/收藏正文；不含 AI 服务、AI 产物与凭据">
        <span className="about-arch-tag">{status ? (status.connected ? '已配置连接' : '未连接') : '状态未确认'}</span>
      </SettingCard>
      {rssConnected && <p className="settings-action-help" role="note">当前使用 RSS 同步服务。断开该服务后可连接 WebDAV。</p>}
      {status && <div className="settings-action-help" role="status" aria-live="polite">
        <div>状态同步确认：{syncTime(status.last_state_sync)}</div>
        <div>正文同步确认：{syncTime(status.last_body_sync)} · 本机待补正文 {status.pending_bodies} 篇</div>
        <div>待处理状态 {status.pending_operations} 项 · 待确认批次 {status.pending_publications} 个</div>
        {status.next_retry_at > 0 && <div>下次重试：{syncTime(status.next_retry_at)}</div>}
        {status.last_error && <div role="alert">最近同步未完成：{status.last_error}</div>}
      </div>}
      <SettingCard title="WebDAV 地址"><input className="setting-input" aria-label="WebDAV 地址" placeholder="https://dav.example.com/path" value={endpoint} onChange={(e) => { touch(); setEndpoint(e.target.value); }} disabled={restoreBusy} /></SettingCard>
      <SettingCard title="WebDAV 用户名"><input className="setting-input" aria-label="WebDAV 用户名" value={username} onChange={(e) => { touch(); setUsername(e.target.value); }} disabled={restoreBusy} autoComplete="username" /></SettingCard>
      <SettingCard title="WebDAV 密码" desc="凭据只保存在本机；修改连接时请重新输入密码"><input className="setting-input" type="password" aria-label="WebDAV 密码" value={password} onChange={(e) => { touch(); setPassword(e.target.value); }} disabled={restoreBusy} autoComplete="new-password" /></SettingCard>
      <SettingCard title="同步时机"><FluxDropdown width={130} value={mode} onChange={(value) => { touch(); setMode(value as LocalSyncMode); }} options={[{ value: 'realtime', label: '实时' }, { value: 'scheduled', label: '定时' }]} /></SettingCard>
      {mode === 'scheduled' && <SettingCard title="同步间隔（分钟）"><input className="setting-input" type="number" min={1} max={1440} step={1} aria-label="WebDAV 同步间隔（分钟）" value={interval} onChange={(e) => { touch(); setInterval(e.target.value); }} /></SettingCard>}
      {(validation || error) && <p className="settings-action-help" role="alert">{validation || error}</p>}
      <div className="settings-action-row">
        <button className="toggle-action-btn btn-primary" disabled={disabled || rssConnected} onClick={() => void connect()}>{busy === 'connect' ? '连接中…' : status?.connected ? '更新连接' : '连接 WebDAV'}</button>
        <button className="toggle-action-btn" disabled={disabled || !status?.connected} onClick={() => void useAppStore.getState().runLocalSync()}>{busy === 'run' ? '状态同步中…' : '现在同步'}</button>
        {status?.connected && <button className="toggle-action-btn" disabled={disabled} onClick={() => void useAppStore.getState().disconnectLocalSync()}>{busy === 'disconnect' ? '断开中…' : '断开 WebDAV'}</button>}
        <button className="toggle-action-btn" disabled={disabled} onClick={() => void refresh().catch(() => {})}>刷新状态</button>
      </div>
    </>
  );
}
