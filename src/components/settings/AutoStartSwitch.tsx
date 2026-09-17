import { useEffect, useState } from 'react';
import { useAppStore } from '../../store';
import { Switch, SettingCard } from '../primitives';

/** 自启动开关：真实读写注册表（tauri-plugin-autostart），设置值只做镜像 */
export function AutoStartSwitch() {
  const settings = useAppStore((s) => s.settings);
  const updateSettings = useAppStore((s) => s.updateSettings);
  const showToast = useAppStore((s) => s.showToast);
  const [regEnabled, setRegEnabled] = useState<boolean | null>(null);

  /* 挂载时读注册表真值（可能与镜像不一致：用户在任务管理器里改过） */
  useEffect(() => {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
    void import('@tauri-apps/plugin-autostart')
      .then(({ isEnabled }) => isEnabled())
      .then(setRegEnabled)
      .catch(() => setRegEnabled(null));
  }, []);

  const toggle = async (v: boolean) => {
    updateSettings({ autoStart: v });
    try {
      const { enable, disable } = await import('@tauri-apps/plugin-autostart');
      if (v) await enable();
      else await disable();
      setRegEnabled(v);
    } catch {
      showToast('自启动设置失败');
      setRegEnabled(!v);
    }
  };

  return (
    <SettingCard title="开机自启动" desc="登录系统后自动在后台启动 FluxReader">
      <Switch checked={regEnabled ?? settings.autoStart} onChange={(v) => void toggle(v)} />
    </SettingCard>
  );
}
