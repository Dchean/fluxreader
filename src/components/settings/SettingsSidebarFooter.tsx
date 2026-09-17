import { useEffect, useState } from 'react';

/** 设置侧栏脚部版本号：动态读取（与 tauri.conf.json 同源，避免硬编码漂移） */
export function SettingsSidebarFooter() {
  const [version, setVersion] = useState('');
  useEffect(() => {
    let alive = true;
    import('@tauri-apps/api/app')
      .then(({ getVersion }) => getVersion())
      .then((v) => alive && setVersion(v))
      .catch(() => alive && setVersion('0.8.0'));
    return () => { alive = false; };
  }, []);
  return <div className="settings-sidebar-footer">{`FluxReader v${version || '…'}`}</div>;
}
