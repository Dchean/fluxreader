// OPT-016B：真实挂载测试的 Tauri 模块桩 loader（harness 注册链的最内层——先于
// ui-loader/test-loader 处理 specifier）。
//
// 只 mock「外部 IPC 边界」：事件监听/窗口控制/应用版本/自启动插件的模块级 API。
// 不 mock 应用自身逻辑；@tauri-apps/api/core（invoke/Channel）的 mock 仍由既有
// tools/test-loader.mjs 单点提供（本文件不重复实现，避免第二份 mock 分叉）。
//
// 为什么需要：App.tsx 挂载时会动态 import '@tauri-apps/api/event' 建立监听；
// 真实模块经 __TAURI_INTERNALS__.transformCallback 走真实 IPC 协议，在 jsdom
// 下会以未捕获异常形态失败。桩成「注册即成功、永不触发」——监听是否建立不是
// 本测试的取证对象，而是被 mock 掉的环境边界。

const MOCKS = {
  /* 覆盖 test-loader 的 plugin-opener 无操作桩：记录打开的 URL，供「正文 <a>
     点击真的走到 opener」类断言取证（test-loader 保持既有 mock 不动，两条
     测试通道互不影响）。 */
  '@tauri-apps/plugin-opener': `
export async function openUrl(url) {
  (globalThis.__OPEN_URLS__ ??= []).push(String(url));
}
export async function revealItemInDir() {}
`,
  '@tauri-apps/api/event': `
export async function listen() { return () => {}; }
export async function once() { return () => {}; }
export async function emit() { return; }
export async function emitTo() { return; }
export class TauriEvent {
  static get TauriEvent() { return undefined; }
}
`,
  '@tauri-apps/api/window': `
export function getCurrentWindow() {
  return {
    minimize: async () => {},
    toggleMaximize: async () => {},
    close: async () => {},
    isMaximized: async () => false,
    setTitle: async () => {},
  };
}
`,
  '@tauri-apps/api/app': `
export async function getVersion() { return '0.0.0-test'; }
export async function getName() { return 'fluxreader-test'; }
export async function getTauriVersion() { return '2.0.0-test'; }
`,
  '@tauri-apps/plugin-autostart': `
export async function isEnabled() { return false; }
export async function enable() {}
export async function disable() {}
`,
  '@tauri-apps/plugin-notification': `
export async function isPermissionGranted() { return false; }
export async function requestPermission() { return 'denied'; }
export function sendNotification() {}
`,
};

export async function resolve(specifier, context, nextResolve) {
  if (specifier in MOCKS) return { url: 'compat-mock:' + specifier, shortCircuit: true };
  return nextResolve(specifier, context);
}

export async function load(url, context, nextLoad) {
  if (url.startsWith('compat-mock:')) {
    const key = url.slice('compat-mock:'.length);
    return { format: 'module', source: MOCKS[key], shortCircuit: true };
  }
  return nextLoad(url, context);
}
