import { useAppStore } from '../../store';
import { isTauri } from '../../lib/api';
import { SettingCard } from '../primitives';

/** 预览/任务/结果由 store 持有；切页或关闭设置不会丢失已提交的恢复结果。 */
export function PortableBackupSection() {
  const state = useAppStore((s) => s.portableBackup);
  const choose = useAppStore((s) => s.choosePortableBackup);
  const restore = useAppStore((s) => s.restorePortableBackup);
  const cancel = useAppStore((s) => s.cancelPortablePreview);
  const retry = useAppStore((s) => s.retryRestoredLibraryRefresh);
  if (!isTauri()) return null;
  const preview = state.preview;
  return <>
    <div className="settings-group-title" style={{ marginTop: 20 }}>便携备份与换机</div>
    <SettingCard title="导出便携备份" desc="包含订阅、分类、普通设置与全部收藏（含已退订来源）；不含 AI 产物、AI 服务与凭据">
      <button className="toggle-action-btn" disabled={!!state.busy || state.refreshFailed} onClick={() => void choose(true)}>{state.busy === 'export' ? '导出中…' : '导出 .flxb 文件'}</button>
    </SettingCard>
    <SettingCard title="恢复便携备份" desc="先校验并预览；确认后替换当前资料库，断开同步服务并重新输入凭据">
      <button className="toggle-action-btn" disabled={!!state.busy || state.refreshFailed} onClick={() => void choose(false)}>{state.busy === 'preview' ? '校验中…' : '选择备份并预览'}</button>
    </SettingCard>
    {preview && <section className="setting-card config-plan-preview" aria-label="便携备份预览">
      <div className="setting-card-text"><h5>恢复预览</h5><p>{new Date(preview.info.created_at).toLocaleString()}</p></div>
      <div className="config-plan-body">
        <code className="database-backup-path">{preview.path}</code>
        <p>订阅 {preview.info.sources} 个 · 分类 {preview.info.folders} 个 · 收藏 {preview.info.starred} 篇</p>
        <p>文章 {preview.info.articles} 篇 · 可用正文 {preview.info.protected_bodies} 篇 · 缺少正文 {preview.info.missing_bodies} 篇</p>
        <p className="config-plan-danger">确认后替换当前资料库；RSS 与 WebDAV 均会断开，AI 服务与所有凭据需要重新输入。</p>
        <p>本机开机启动设置保留。缺少的正文不会因恢复自动变成可读。</p>
        <div className="settings-action-row">
          <button className="toggle-action-btn btn-danger-text" disabled={!!state.busy} onClick={() => void restore()}>确认替换并恢复</button>
          <button className="toggle-action-btn" disabled={!!state.busy} onClick={cancel}>取消</button>
        </div>
      </div>
    </section>}
    {state.result && <div className="settings-action-help database-backup-result" role="status">
      <p>{state.result.kind === 'export' ? '便携备份已导出' : '资料库恢复已完成'} · 订阅 {state.result.info.sources} 个 · 收藏 {state.result.info.starred} 篇 · 缺正文 {state.result.info.missing_bodies} 篇</p>
      <code className="database-backup-path">{state.result.info.path}</code>
    </div>}
    {state.error && <p className="settings-action-help database-backup-error" role="alert">{state.error}</p>}
    {state.refreshFailed && <button className="toggle-action-btn" disabled={!!state.busy} onClick={() => void retry()}>重试界面刷新</button>}
  </>;
}
