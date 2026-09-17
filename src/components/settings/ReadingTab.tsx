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
      <SettingCard title="正文字体" desc="选择阅读器正文渲染字体家族">
        <FluxDropdown
          width={200}
          value={settings.fontFamily}
          onChange={(v) => updateSettings({ fontFamily: v })}
          options={FONT_OPTIONS}
        />
      </SettingCard>
      <SettingCard title="字号" desc="调整正文基础显示大小">
        <div className="range-slider-wrap">
          <input
            type="range"
            min={13}
            max={24}
            value={settings.fontSize}
            className="range-input"
            onChange={(e) => updateSettings({ fontSize: Number(e.target.value) })}
          />
          <span style={{ width: 45 }}>{settings.fontSize}px</span>
        </div>
      </SettingCard>
      <SettingCard title="行高" desc="调整正文段落行间距比例">
        <div className="range-slider-wrap">
          <input
            type="range"
            min={130}
            max={240}
            value={settings.lineHeight}
            className="range-input"
            onChange={(e) => updateSettings({ lineHeight: Number(e.target.value) })}
          />
          <span style={{ width: 45 }}>{settings.lineHeight}%</span>
        </div>
      </SettingCard>

      <div className="settings-group-title">版面</div>
      <SettingCard title="正文最大宽度" desc="限制单行文本长度以优化可读性">
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
          <span style={{ width: 55 }}>{settings.maxWidth}px</span>
        </div>
      </SettingCard>
      <SettingCard title="显示预计阅读时间" desc="在文章信息栏显示估算阅读时长">
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
