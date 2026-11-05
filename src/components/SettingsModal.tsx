import { useRef } from 'react';
import { useAppStore } from '../store';
import { ModalOverlay } from './primitives';
import { useEnteringClass } from './useEnteringClass';
import { TAB_META } from './settings/shared';
import { GeneralTab } from './settings/GeneralTab';
import { AppearanceTab } from './settings/AppearanceTab';
import { ReadingTab } from './settings/ReadingTab';
import { FeedsTab } from './settings/FeedsTab';
import { AiTab } from './settings/AiTab';
import { SyncTab } from './settings/SyncTab';
import { ShortcutsTab } from './settings/ShortcutsTab';
import { SettingsSidebarFooter } from './settings/SettingsSidebarFooter';
import { AboutTab } from './settings/AboutTab';

/* ============================================================
   设置中心 —— 沉浸式双栏布局，左侧导航 8 页签
   ============================================================ */

export function SettingsModal() {
  const settingsOpen = useAppStore((s) => s.settingsOpen);
  const settingsTab = useAppStore((s) => s.settingsTab);
  const closeSettings = useAppStore((s) => s.closeSettings);
  const switchSettingsTab = useAppStore((s) => s.switchSettingsTab);

  /* 切换左侧页签时右侧内容区淡入一次（REQ-005） */
  const paneRef = useRef<HTMLDivElement>(null);
  useEnteringClass(paneRef, settingsTab, 'pane-entering');

  const meta = TAB_META.find((t) => t.id === settingsTab) ?? TAB_META[0];

  return (
    <ModalOverlay open={settingsOpen} onClose={closeSettings} ariaLabel="设置">
      <div className="settings-modal" onClick={(e) => e.stopPropagation()}>
        {/* 左侧导航 */}
        <div className="settings-sidebar">
          <div className="settings-sidebar-header">
            <span className="settings-title-text">设置</span>
            <span className="kbd-tag">Ctrl+,</span>
          </div>
          <nav className="settings-nav">
            {TAB_META.map((m) => (
              <button
                key={m.id}
                className={`settings-nav-item ${settingsTab === m.id ? 'active' : ''}`}
                onClick={() => switchSettingsTab(m.id)}
                aria-current={settingsTab === m.id ? 'page' : undefined}
              >
                <m.icon />
                <span>{m.title}</span>
              </button>
            ))}
          </nav>
          <SettingsSidebarFooter />
        </div>

        {/* 右侧内容区 */}
        <div className="settings-content-area">
          <div className="settings-content-header">
            <h2 className="settings-pane-title">{meta.title}</h2>
            <div className="settings-pane-header-right">
              <span className="settings-pane-subtitle">{meta.subtitle}</span>
              <button className="win-btn close" onClick={closeSettings} title="关闭设置 (Esc)">✕</button>
            </div>
          </div>

          <div className="settings-content-pane" ref={paneRef}>
            {settingsTab === 'general' && <GeneralTab />}
            {settingsTab === 'appearance' && <AppearanceTab />}
            {settingsTab === 'reading' && <ReadingTab />}
            {settingsTab === 'feeds' && <FeedsTab />}
            {settingsTab === 'ai' && <AiTab />}
            {settingsTab === 'sync' && <SyncTab />}
            {settingsTab === 'shortcuts' && <ShortcutsTab />}
            {settingsTab === 'about' && <AboutTab />}
          </div>
        </div>
      </div>
    </ModalOverlay>
  );
}
