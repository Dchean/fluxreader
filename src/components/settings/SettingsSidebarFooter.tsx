import { useEffect, useState } from 'react';

/** 设置侧栏脚部版本号：动态读取（与 tauri.conf.json 同源，避免硬编码漂移） */
export function SettingsSidebarFooter() {
  const [version, setVersion] = useState('');
  useEffect(() => {
    let alive = true;
    import('@tauri-apps/api/app')
      .then(({ getVersion }) => getVersion())
      .then((v) => alive && setVersion(v))
      /* fix-14（自检 UI-P2-8）：获取失败保持 '…'——不回退硬编码假版本号
         （与 AboutTab 同一决策：假版本号比无版本号更有害）。 */
      .catch(() => { /* 保持 version='' → 页脚显示 FluxReader v… */ });
    return () => { alive = false; };
  }, []);
  return <div className="settings-sidebar-footer">{`FluxReader v${version || '…'}`}</div>;
}
