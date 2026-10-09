// OPT-016B 真实挂载回归入口（Node 内置 test 运行）。
//
// 运行：npm run test:components
//   = tsc -p tsconfig.test.json && node ./tools/component-regression.mjs
//
// 与 tools/frontend-regression.mjs（纯逻辑状态机回归）分工：
// - 旧通道不挂 DOM、不跑 effect——保护 store/selector 行为；
// - 本通道用 jsdom + React createRoot/act 真实挂载生产组件（ReaderProse/Reader、
//   Timeline 五布局、CloseAskDialog/App 快捷键、卡片正文水合），保护 effect/
//   事件/异步链与真实 DOM 契约。
// 两条通道共用同一份 loader 实现（test-loader/ui-loader），不复制第二套。
//
// 证据边界（不做成 SSR 冒充，也不夸大）：
// - jsdom 无布局/网络：ResizeObserver、matchMedia、offsetHeight/getBoundingClientRect、
//   scrollTo/scrollIntoView、DOMParser 做最小环境替身（harness.mjs）；
// - 外部 IPC 全部 fake（test-loader 的 @tauri-apps/api/core mock + 本目录
//   tauri-compat-loader 对 event/window/app/autostart 的无副作用桩），真实 Tauri
//   运行时与真实网络零接触；
// - 挂载的是生产组件与 dist-test 的同一份 store/bodyCache，不做组件替身。
//
// 生命周期协议（R1 修复）：
// - ref 看门狗主动保活：用例/after 钩子 pending 或清理未完成 → 到期 exit 3，
//   不再让悬挂被事件循环空转伪装成绿灯（旧 unref 实现实测 `after(() => new
//   Promise(() => {}))` 仍 exit 0）；
// - 最终收尾 runFinalTeardown：清理必完成、错误账本终检、失败显式置
//   process.exitCode=1 后再抛（node:test programmatic 模式不因 hook 失败改进程码）；
// - 全部用例经 harness.componentTest 包装：body/清理/账本三者失败聚合、任一即红。

import { after } from 'node:test';
import { armLifecycleWatchdog, disarmLifecycleWatchdog, runFinalTeardown } from './component-tests/harness.mjs';

/* ref 看门狗：正常结束由 runFinalTeardown 显式解除；任何 pending 悬挂到期强杀。 */
armLifecycleWatchdog();

/* 用例文件按「先最小可运行、后复杂链路」顺序注册（node:test 在 import 时登记）。 */
await import('./component-tests/reader-prose.test.mjs');
await import('./component-tests/layouts.test.mjs');
await import('./component-tests/close-ask.test.mjs');
await import('./component-tests/hydrate.test.mjs');
await import('./component-tests/mutation-reader-prose.test.mjs');
await import('./component-tests/runner-selfcheck.test.mjs');

/* 全部用例结束后统一收尾；失败非静默、退出码非 0；无论成败都解除看门狗
   （pending 场景下 finally 不会到达 → 看门狗按期强杀，这正是设计意图）。 */
after(async () => {
  try {
    await runFinalTeardown();
  } finally {
    disarmLifecycleWatchdog();
  }
});
