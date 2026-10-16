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
/* OPT-017C：画廊有界渲染——5000 项挂载数上界/键盘焦点/灯箱返回/加载更多/列分配。 */
await import('./component-tests/gallery-virtual.test.mjs');
await import('./component-tests/close-ask.test.mjs');
await import('./component-tests/hydrate.test.mjs');
await import('./component-tests/mutation-reader-prose.test.mjs');
/* OPT-008B R2（主控清单②）：搜索跨布局结果——真实 SearchModal 点击打开 +
   Reader/布局实际呈现 + 返回/续页边界（数组存在不能自证渲染可见）。 */
await import('./component-tests/search-cross-layout.test.mjs');
/* OPT-008B R3（主控清单①）：Reader 主动修订恢复——无 Timeline 卡片也自动重水合；
   在途旧响应丢戳后新响应落地。 */
await import('./component-tests/reader-revision-recovery.test.mjs');
/* OPT-008B R3（主控清单⑥）：五布局共享正文修订——失效清空 + 真实重水合 +
   Reader/卡片渲染证据（social 含同窗口兄弟条目）。 */
await import('./component-tests/revision-shared-layouts.test.mjs');
/* OPT-007（F11）：feeds-updated 数据变化事件 → 前端真实监听器 → 保位 reload。 */
await import('./component-tests/sync-data-events.test.mjs');
/* OPT-011（F13）：AI 开关三态——Reader 自动请求订阅有效配置 + FeedsTab 三态控件。 */
await import('./component-tests/ai-settings.test.mjs');
/* OPT-013 B1：AI 产物缓存签名——「未标记版本」标记、未命中不短路（真实点击发
   起生成）、命中不重复计费、同键在途只发一次。 */
await import('./component-tests/ai-cache-signature.test.mjs');
/* OPT-011 R5：配置应用/上传经真实 UI 接入同一 AI 写协议（写屏障/前沿等待/回读确认）。 */
await import('./component-tests/config-apply.test.mjs');
/* OPT-009 B3：订阅写回能力声明与持久回执（FeedsTab 确认文案、Fever 仅本地、
   subscription-outbox-changed 事件链驱动摘要卡）。 */
await import('./component-tests/subscription-writeback.test.mjs');
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
