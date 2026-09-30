import { useAppStore } from '../../store';
import { FluxDropdown, Switch, SettingCard } from '../primitives';
import { FONT_OPTIONS } from './shared';

/* ---------- TAB 3: 阅读 ---------- */

export function ReadingTab() {
  const settings = useAppStore((s) => s.settings);
  const updateSettings = useAppStore((s) => s.updateSettings);

  return (
    <>
      <div className="settings-group-title">字体</div>
      {/* TASK-102：字体/字号/行高/最大宽度/阅读时间的标题已自解释，
          desc 均复述标题 → 删除（仅保留有触发语义的两条） */}
      <SettingCard title="正文字体">
        <FluxDropdown
          width={200}
          value={settings.fontFamily}
          onChange={(v) => updateSettings({ fontFamily: v })}
          options={FONT_OPTIONS}
        />
      </SettingCard>
      <SettingCard title="字号">
        <div className="range-slider-wrap">
          <input
            type="range"
            min={13}
            max={24}
            value={settings.fontSize}
            className="range-input"
            onChange={(e) => updateSettings({ fontSize: Number(e.target.value) })}
          />
          <span className="range-value-tag">{settings.fontSize}px</span>
        </div>
      </SettingCard>
      <SettingCard title="行高">
        <div className="range-slider-wrap">
          <input
            type="range"
            min={130}
            max={240}
            value={settings.lineHeight}
            className="range-input"
            onChange={(e) => updateSettings({ lineHeight: Number(e.target.value) })}
          />
          <span className="range-value-tag">{settings.lineHeight}%</span>
        </div>
      </SettingCard>

      <div className="settings-group-title">版面</div>
      <SettingCard title="正文最大宽度">
        <div className="range-slider-wrap">
          <input
            type="range"
            min={560}
            max={1100}
            step={20}
            value={settings.maxWidth}
            className="range-input"
            onChange={(e) => updateSettings({ maxWidth: Number(e.target.value) })}
          />
          <span className="range-value-tag">{settings.maxWidth}px</span>
        </div>
      </SettingCard>
      <SettingCard title="显示预计阅读时间">
        <Switch checked={settings.showReadTime} onChange={(v) => updateSettings({ showReadTime: v })} />
      </SettingCard>

      <div className="settings-group-title">打开方式</div>
      <SettingCard title="默认打开方式" desc="遇到部分未提供全文的 RSS 订阅源时自动执行正文提取">
        <FluxDropdown
          width={140}
          value={settings.defaultOpenMode}
          onChange={(v) => updateSettings({ defaultOpenMode: v as 'rss' | 'fulltext' })}
          options={[
            { value: 'rss', label: 'RSS 正文' },
            { value: 'fulltext', label: '自动全文' },
          ]}
        />
      </SettingCard>
      <SettingCard title="智能去重" desc="同一篇文章被多个订阅源推送时只保留最先入库的一份">
        <Switch checked={settings.smartDedup} onChange={(v) => updateSettings({ smartDedup: v })} />
      </SettingCard>
    </>
  );
}
