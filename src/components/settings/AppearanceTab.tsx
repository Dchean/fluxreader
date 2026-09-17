import { useAppStore } from '../../store';
import { PALETTES } from './shared';

/* ---------- TAB 2: 外观 ---------- */

export function AppearanceTab() {
  const settings = useAppStore((s) => s.settings);
  const updateSettings = useAppStore((s) => s.updateSettings);

  return (
    <>
      <div className="settings-group-title">主题模式</div>
      <div className="theme-mode-grid">
        {([
          { id: 'light', label: '☀️ 浅色模式' },
          { id: 'dark', label: '🌙 深色模式' },
          { id: 'auto', label: '💻 跟随系统' },
        ] as const).map((m) => (
          <button
            key={m.id}
            className={`toggle-action-btn theme-mode-btn ${settings.themeMode === m.id ? 'theme-active' : ''}`}
            onClick={() => updateSettings({ themeMode: m.id })}
          >
            {m.label}
          </button>
        ))}
      </div>

      <div className="settings-group-title">全局配色方案</div>
      <div className="palette-swatches-grid">
        {PALETTES.map((p) => (
          <button
            key={p.id}
            className={`palette-card-btn ${settings.palette === p.id ? 'active' : ''}`}
            onClick={() => updateSettings({ palette: p.id })}
          >
            <div className="palette-dot" style={{ background: p.color }} />
            <span className="palette-name">{p.name}</span>
          </button>
        ))}
      </div>
    </>
  );
}
