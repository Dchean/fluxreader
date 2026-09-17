import { useAppStore } from '../../store';
import { STARTUP_VIEW_OPTIONS } from '../../types';
import { FluxDropdown, Switch, SettingCard } from '../primitives';
import { AutoStartSwitch } from './AutoStartSwitch';

/* ---------- TAB 1: 通用 ---------- */

export function GeneralTab() {
  const settings = useAppStore((s) => s.settings);
  const updateSettings = useAppStore((s) => s.updateSettings);

  return (
    <>
      <div className="settings-group-title">刷新</div>
      <SettingCard title="自动刷新" desc="后台定期检查订阅源更新">
        <Switch checked={settings.autoRefresh} onChange={(v) => updateSettings({ autoRefresh: v })} />
      </SettingCard>
      <SettingCard title="刷新间隔" desc="时间越短，电池消耗越大">
        <div className="range-slider-wrap">
          <input
            type="range"
            min={5}
            max={120}
            value={settings.refreshInterval}
            className="range-input"
            onChange={(e) => updateSettings({ refreshInterval: Number(e.target.value) })}
          />
          <span className="range-value-tag">{settings.refreshInterval} 分钟</span>
        </div>
      </SettingCard>
      <SettingCard title="并发抓取数" desc="同时抓取的源数量。源多可调高，被限流时调低。">
        <div className="range-slider-wrap">
          <input
            type="range"
            min={1}
            max={16}
            step={1}
            value={settings.fetchConcurrency}
            className="range-input"
            onChange={(e) => updateSettings({ fetchConcurrency: Number(e.target.value) })}
          />
          <span className="range-value-tag">{settings.fetchConcurrency} 路</span>
        </div>
      </SettingCard>

      <div className="settings-group-title">已读行为</div>
      <SettingCard title="打开文章时标为已读" desc="点击选中文章后立即更新本地已读状态">
        <Switch checked={settings.markReadOnOpen} onChange={(v) => updateSettings({ markReadOnOpen: v })} />
      </SettingCard>
      <SettingCard title="滚动到底部时标为已读" desc="正文滚至末尾才算读过，适合深度阅读">
        <Switch checked={settings.markReadOnScrollBottom} onChange={(v) => updateSettings({ markReadOnScrollBottom: v })} />
      </SettingCard>
      <SettingCard title="滚动出列表区域时标为已读" desc="卡片滚出时间流上沿即视为已浏览">
        <Switch checked={settings.markReadOnScrollOut} onChange={(v) => updateSettings({ markReadOnScrollOut: v })} />
      </SettingCard>

      <div className="settings-group-title">启动</div>
      <AutoStartSwitch />
      <SettingCard title="关闭时最小化到托盘" desc="关闭按钮仅隐藏窗口，托盘图标常驻；托盘菜单「退出」才是真正退出">
        <Switch checked={settings.closeToTray} onChange={(v) => updateSettings({ closeToTray: v })} />
      </SettingCard>
      <SettingCard title="新文章系统通知" desc="窗口在后台/托盘时，后台刷新抓到新文章发 Windows 通知">
        <Switch checked={settings.notifyOnNewArticles} onChange={(v) => updateSettings({ notifyOnNewArticles: v })} />
      </SettingCard>
      <SettingCard title="启动时打开" desc="下次打开应用时默认进入的视图">
        {/* D4：选项与 store 的启动白名单（STARTUP_VIEWS）同源——此前的
            「文章」('article') 不在白名单内，选中后静默失效，已移除；
            白名单里的「收藏」补上了 UI 入口。 */}
        <FluxDropdown
          width={130}
          value={settings.startupView}
          onChange={(v) => updateSettings({ startupView: v })}
          options={STARTUP_VIEW_OPTIONS}
        />
      </SettingCard>
      <SettingCard title="启动时隐藏已读" desc="仅展示未读流内容">
        <Switch checked={settings.hideReadOnStartup} onChange={(v) => updateSettings({ hideReadOnStartup: v })} />
      </SettingCard>
    </>
  );
}
