// OPT-016B 真实挂载测试公共 harness（tools/component-regression.mjs 是其唯一入口）。
//
// 为什么需要它：tools/frontend-regression.mjs 是纯逻辑回归（Node 直跑 store），组件侧
// 只有 SSR（react-dom/server）形态断言——SSR 不跑 effect、不发事件、没有真实 DOM，
// 「源码态真的不创建元素」「五布局切换真的换卡片 DOM」「确认框打开时键盘真的不改背
// 后文章」都无法在 SSR 下取证。本 harness 用 jsdom 建立真实 DOM，用 React
// createRoot + act 真实挂载生产组件，effect/事件/异步按真实路径执行。
//
// 证据边界（如实说明，不夸大）：
// - jsdom 不是 WebView2：无布局引擎、无网络栈。本 harness 对「缺失的浏览器 API」
//   （ResizeObserver / matchMedia / getBoundingClientRect / scrollTo / scrollIntoView）
//   做最小替身；这些替身只提供尺寸与滚动语义，不替身任何应用逻辑。
// - 外部 IPC（@tauri-apps/api/core）经 tools/test-loader.mjs 的既有 mock 指向
//   globalThis.__INVOKE__；本目录的 tauri-compat-loader.mjs 另对 event/window/
//   app/autostart 模块做无副作用桩。真实 Tauri 运行时、真实网络一律不触碰。
// - store 只有一个实例：组件经 tools/ui-loader.mjs 的既有别名（src/store.ts →
//   dist-test/store.js）拿到的就是本 harness 断言的同一份 zustand store 与同一份
//   bodyCache（断言它的行为而不只是声称它——见各用例）。
//
// 无悬挂纪律（R1 强化）：每个用例自带 node:test timeout；组件统一经 mount() 登记、
// 每用例结束与最终清理都经 unmountAll()/teardownAll() 卸载；入口另持一把**ref 的
// 生命周期看门狗**（主动保活：用例/after 钩子 pending 或清理未完成时强制非 0 退出，
// 不再让悬挂被「事件循环空转」伪装成正常退出），并在清理完成后显式解除。

import { register } from 'node:module';
import { fileURLToPath } from 'node:url';
import { writeSync } from 'node:fs';
import { test } from 'node:test';
import { JSDOM, VirtualConsole } from 'jsdom';

export const REPO_ROOT = fileURLToPath(new URL('../../', import.meta.url));

/* ------------------------------------------------------------------
   0) 终态运行时错误守卫（R2 修复）：finalcheck 之后到达的异步运行时错误
      （典型：真实 React effect cleanup 里 setTimeout 晚发的 console.error）
      此前只被采集器记录、无人再查 → 退出码 0 假绿。守卫在 runFinalTeardown
      通过终检后武装；三条通道（console/jsdom/React）一致，任何新错误立即：
      - 同步写清晰诊断（writeSync 直写 fd 2，不依赖被接管的 console、不丢输出）
      - process.exitCode = 1（不 sleep、不等待、不再有「finalize 后掩盖」窗口）
      one-shot 例外只作用于用例的预期窗口；武装后不再参考任何过时例外。
   ------------------------------------------------------------------ */
let runtimeGuardArmed = false;

export function armRuntimeErrorGuard() { runtimeGuardArmed = true; }

export function dischargeRuntimeErrorGuard() { runtimeGuardArmed = false; }

function emitDiagnostic(line) {
  try { writeSync(2, `${line}\n`); }
  catch { try { process.stderr.write(`${line}\n`); } catch { /* 无可用输出面 */ } }
}

function tripRuntimeErrorGuard(channel, message) {
  if (!runtimeGuardArmed) return;
  emitDiagnostic(`[component-regression] 终态运行时错误（finalize 之后不允许再出现未预期错误；进程退出码置 1）：${channel}: ${message}`);
  process.exitCode = 1;
}

/* ------------------------------------------------------------------
   1) 先固定 cwd：ui-loader 的 store 别名用相对路径 'dist-test/store.js' 解析，
      运行目录不是仓库根时别名会指向错误位置。测试从任何 cwd 启动都收敛到这里。
   ------------------------------------------------------------------ */
process.chdir(REPO_ROOT);

/* ------------------------------------------------------------------
   2) jsdom DOM 环境。pretendToBeVisual 提供 requestAnimationFrame；
      VirtualConsole 不接监听器 = jsdom 的 "Not implemented"（如 window.scrollTo）
      不会污染输出，但保留在 jsdomErrors 里供诊断——不静默吞真实异常。
   ------------------------------------------------------------------ */
export const jsdomErrors = [];
const virtualConsole = new VirtualConsole();
virtualConsole.on('jsdomError', (e) => {
  const msg = String(e?.message ?? e);
  jsdomErrors.push(msg);
  tripRuntimeErrorGuard('jsdom 未捕获', msg);
});

export const dom = new JSDOM(
  '<!doctype html><html><head></head><body><div id="root"></div></body></html>',
  { url: 'http://localhost/', pretendToBeVisual: true, virtualConsole },
);

const w = dom.window;

/* 组件代码经裸全局名访问浏览器对象（window/document/ResizeObserver/…），
   故必须逐项挂到 globalThis；只挂 window 本身不够。 */
globalThis.window = w;
globalThis.document = w.document;
globalThis.self = w;
globalThis.Node = w.Node;
globalThis.Element = w.Element;
globalThis.HTMLElement = w.HTMLElement;
globalThis.Event = w.Event;
globalThis.CustomEvent = w.CustomEvent;
globalThis.MouseEvent = w.MouseEvent;
globalThis.KeyboardEvent = w.KeyboardEvent;
globalThis.FocusEvent = w.FocusEvent;
globalThis.MutationObserver = w.MutationObserver;
/* imageProxy 用 DOMParser 做正文 HTML 的代理替换（src/lib/imageProxy.ts）——
   jsdom 自带实现，直接暴露为全局（属「缺失的浏览器 API」补齐，不替身逻辑）。 */
globalThis.DOMParser = w.DOMParser;
globalThis.getComputedStyle = w.getComputedStyle.bind(w);
globalThis.requestAnimationFrame = w.requestAnimationFrame.bind(w);
globalThis.cancelAnimationFrame = w.cancelAnimationFrame.bind(w);
/* 注意：不得把 globalThis.performance 换成 jsdom 的——jsdom 的 Performance 实现
   内部读全局 performance.now()，被替换后会自引用递归爆栈（实测 Maximum call
   stack）。Node 自带的 performance.now 单调时钟满足组件用法。 */
globalThis.localStorage = w.localStorage;
/* Node 21+ 的 globalThis.navigator 是 getter；jsdom 的 navigator 在个别
   exports 里会被读 userAgent——尽力覆盖，失败不致命。 */
try {
  Object.defineProperty(globalThis, 'navigator', { value: w.navigator, configurable: true, writable: true });
} catch { /* 覆盖失败：保留 Node 自带 navigator */ }

/* Tauri 检测（src/lib/api.ts isTauri）读的是 window 上的 __TAURI_INTERNALS__ 键。
   这里给空对象：invoke 路径走 test-loader 的 core mock（不看 internals），
   App.tsx 动态 import 的 event/window 模块被 tauri-compat-loader.mjs 换成桩。 */
w.__TAURI_INTERNALS__ = {};

/* React 19 的 act 需要显式测试环境标记。 */
globalThis.IS_REACT_ACT_ENVIRONMENT = true;

/* ------------------------------------------------------------------
   3) 缺失浏览器 API 的最小替身（只补环境，不替身应用逻辑）
   ------------------------------------------------------------------ */

/** 统一尺寸替身：jsdom 无布局引擎，所有元素 getBoundingClientRect 恒 0。
    虚拟滚动（@tanstack/react-virtual）依赖它测视口与行高——返回固定视口
    (1024×768) + 行高 (160px，与 Timeline estimateSize 一致)，让视口内/overscan
    的卡片真实进入 DOM。data-test-height 允许个别元素定制（本地用例用不到）。 */
class Rect {
  constructor(width, height) {
    this.width = width; this.height = height;
    this.top = 0; this.left = 0; this.right = width; this.bottom = height;
    this.x = 0; this.y = 0;
  }
  toJSON() { return { width: this.width, height: this.height }; }
}
const rectOverrides = new WeakMap();
w.Element.prototype.getBoundingClientRect = function getBoundingClientRect() {
  const custom = rectOverrides.get(this);
  if (custom) return custom;
  return new Rect(1024, defaultOffsetHeight(this));
};
export function setElementRect(el, width, height) { rectOverrides.set(el, new Rect(width, height)); }

/** offsetHeight/offsetWidth 替身：@tanstack/virtual-core 的 getRect/measureElement
    读的是 offsetHeight（不是 getBoundingClientRect），jsdom 恒 0 会让虚拟列表
    永远渲染 0 项——必须按语义补：滚动容器 = 视口高，虚拟行 = 估算行高。 */
const offsetOverrides = new WeakMap();
function defaultOffsetHeight(el) {
  if (el.id === 'timelineContentScroll') return 768;      // 列表滚动容器视口
  if (el.classList?.contains('timeline-virtual-item')) return 160; // 与 estimateSize 同值
  return 0;
}
Object.defineProperty(w.HTMLElement.prototype, 'offsetHeight', {
  configurable: true,
  get() { return offsetOverrides.get(this)?.height ?? defaultOffsetHeight(this); },
});
Object.defineProperty(w.HTMLElement.prototype, 'offsetWidth', {
  configurable: true,
  get() { return offsetOverrides.get(this)?.width ?? 1024; },
});
export function setOffsetSize(el, width, height) { offsetOverrides.set(el, { width, height }); }

/** ResizeObserver 替身：observe 时同步回调一次（jsdom 无 resize），
    回调形态与浏览器一致（entries[0].contentRect）——Timeline 的社交卡测高与
    virtual-core 的视口观测都消费它。 */
class ResizeObserverMock {
  constructor(cb) { this._cb = cb; }
  observe(el) {
    const r = typeof el.getBoundingClientRect === 'function' ? el.getBoundingClientRect() : { width: 0, height: 0 };
    const contentRect = { width: r.width, height: r.height, top: r.top ?? 0, left: r.left ?? 0, right: r.right ?? r.width, bottom: r.bottom ?? r.height };
    this._cb([{ target: el, contentRect }], this);
  }
  unobserve() {}
  disconnect() {}
}
w.ResizeObserver = ResizeObserverMock;
globalThis.ResizeObserver = ResizeObserverMock;

/** matchMedia 替身（App 主题 effect 在 themeMode='auto' 时读；默认 dark 不走）。 */
w.matchMedia = (query) => ({
  matches: false, media: query, onchange: null,
  addEventListener() {}, removeEventListener() {},
  addListener() {}, removeListener() {},
  dispatchEvent() { return false; },
});
globalThis.matchMedia = w.matchMedia;

/** Element.scrollTo / scrollIntoView：jsdom 未实现（调用会打 jsdomError）。
    scrollTo 只落 scrollTop/scrollLeft——程序性滚动抑制窗口、锚偏移补加等
    真实的滚动副作用仍按生产代码执行。 */
if (typeof w.Element.prototype.scrollTo !== 'function') {
  w.Element.prototype.scrollTo = function scrollTo(opts, y) {
    if (typeof opts === 'object' && opts !== null) {
      if (typeof opts.top === 'number') this.scrollTop = opts.top;
      if (typeof opts.left === 'number') this.scrollLeft = opts.left;
    } else if (typeof opts === 'number') {
      this.scrollLeft = opts;
      if (typeof y === 'number') this.scrollTop = y;
    }
  };
}
if (typeof w.Element.prototype.scrollIntoView !== 'function') {
  w.Element.prototype.scrollIntoView = function scrollIntoView() { /* no-op：无布局 */ };
}

/* ------------------------------------------------------------------
   4) loader 注册（后注册者先跑）：test-loader（tauri core/opener mock + dist-test
      相对路径补 .js）→ ui-loader（src/*.tsx 就地转译 + store 同实例别名）→
      tauri-compat-loader（event/window/app/autostart 无副作用桩）。
   ------------------------------------------------------------------ */
register(new URL('../test-loader.mjs', import.meta.url).href, new URL('../../', import.meta.url).href);
register(new URL('../ui-loader.mjs', import.meta.url).href, new URL('../../', import.meta.url).href);
register(new URL('./tauri-compat-loader.mjs', import.meta.url).href, new URL('../../', import.meta.url).href);

/* ------------------------------------------------------------------
   5) 同一 store/bodyCache 实例（dist-test 产物——组件经 ui-loader 别名拿到的
      是同一模块）。测试只从这里取实例，不 import src/store.ts（那会经
      ui-loader 重新转译出第二个实例，断言将失去意义）。
   ------------------------------------------------------------------ */
export const storeMod = await import('../../dist-test/store.js');
export const bodyCacheMod = await import('../../dist-test/store/bodyCache.js');
export const useAppStore = storeMod.useAppStore;
export const getBodyEntry = bodyCacheMod.getBodyEntry;
export const resetBodyCacheForTests = bodyCacheMod.resetBodyCacheForTests;

/* ------------------------------------------------------------------
   6) 统一错误账本（R1 修复）：console.error / jsdom 未捕获 / React 三通道错误
      ——「未预期即失败；预期错误用显式精确例外登记并消费」。
      此前只查 getSnapshot/Maximum depth 两个 regex（漏其它异常），jsdom 未捕获
      （真实 DOM listener throw 走这里）只记录不参与失败——两者现都并入终检。
   ------------------------------------------------------------------ */
export const consoleErrors = [];
export const consoleWarns = [];
export const reactErrors = [];
const origError = console.error.bind(console);
const formatArgs = (a) => a.map((x) => (x instanceof Error ? `${x.name}: ${x.message}` : String(x))).join(' ');
console.error = (...a) => {
  const msg = formatArgs(a);
  consoleErrors.push(msg);
  tripRuntimeErrorGuard('console.error', msg);
};
console.warn = (...a) => { consoleWarns.push(a.map(String).join(' ')); };

/** 显式精确例外（one-shot）：匹配一条错误即消费；终检时**未消费的例外视为失败**
    ——防止注册宽泛 pattern 把整类错误全屏蔽（「不能全屏蔽」的机械保障）。 */
const allowances = [];
export function allowConsoleError(re) { allowances.push({ kind: 'console', target: null, re, used: false }); }
export function allowJsdomError(re) { allowances.push({ kind: 'jsdom', target: null, re, used: false }); }
export function allowReactError(re, target = null) { allowances.push({ kind: 'react', target, re, used: false }); }

function consumeAllowance(kind, text, target = null) {
  const a = allowances.find((x) => x.kind === kind && !x.used && x.re.test(text) && (x.target === null || x.target === target));
  if (!a) return false;
  a.used = true;
  return true;
}

/* 已检出的账本增量指针：begin/end 推进，避免同一批条目被重复报告。 */
const ledgerPtr = { console: 0, jsdom: 0, react: 0 };

/** 汇总「自上次检查以来」的未预期条目与未消费例外；无问题返回 null。 */
export function detectUnexpectedErrors() {
  const unexpected = [];
  for (const m of consoleErrors.slice(ledgerPtr.console)) {
    if (!consumeAllowance('console', m)) unexpected.push(`console.error: ${m}`);
  }
  for (const m of jsdomErrors.slice(ledgerPtr.jsdom)) {
    if (!consumeAllowance('jsdom', m)) unexpected.push(`jsdom 未捕获: ${m}`);
  }
  for (const e of reactErrors.slice(ledgerPtr.react)) {
    if (!consumeAllowance('react', e.message, e.kind)) unexpected.push(`react ${e.kind}: ${e.message}`);
  }
  ledgerPtr.console = consoleErrors.length;
  ledgerPtr.jsdom = jsdomErrors.length;
  ledgerPtr.react = reactErrors.length;
  const unused = allowances.filter((a) => !a.used).map((a) => `${a.kind} 例外未被消费（预期错误未出现？）: ${a.re}`);
  allowances.length = 0;
  if (unexpected.length === 0 && unused.length === 0) return null;
  return new Error(`未预期错误 ${unexpected.length} 条、未消费例外 ${unused.length} 条：\n${[...unexpected, ...unused].join('\n')}`);
}

/** 用例起始：先检出自上次终检以来的「间隙错误」（不静默），再清空账本与例外，
    使用例内对 consoleErrors/jsdomErrors 的直读断言只见本用例条目。 */
export function beginTestErrors() {
  const gap = detectUnexpectedErrors();
  allowances.length = 0;
  consoleErrors.length = 0;
  consoleWarns.length = 0;
  jsdomErrors.length = 0;
  reactErrors.length = 0;
  ledgerPtr.console = 0;
  ledgerPtr.jsdom = 0;
  ledgerPtr.react = 0;
  return gap;
}

/** 用例终检（最终清理阶段也会再调一次，覆盖清理期新增条目）。 */
export function endTestErrors() {
  return detectUnexpectedErrors();
}

/* ------------------------------------------------------------------
   7) act/挂载/等待工具
   ------------------------------------------------------------------ */
export async function getAct() {
  const { act } = await import('react');
  return act;
}

/** 已挂载 root 登记表：teardownAll 统一卸载（清理纪律，杜绝悬挂挂载树）。 */
const mountedRoots = [];

/** 挂载真实组件；onCaughtError/onUncaughtError/onRecoverableError 收集含
    componentStack 的 errorInfo（捕获路径的可用性由 fixtures 用例的 canary 证明，
    不是「装了收集器就一定收到」的自证）。
    captureRenderErrors：dev 模式下 React 会把未捕获错误重新抛出（即使已调
    onUncaughtError），canary 用例用该开关吞掉重抛、只消费收集器里的记录。 */
export async function mount(element, { options, captureRenderErrors = false } = {}) {
  const { createRoot } = await import('react-dom/client');
  const container = w.document.createElement('div');
  w.document.body.appendChild(container);
  const records = { uncaught: [], caught: [], recoverable: [], renderError: null };
  const info = (list, kind) => (error, errorInfo) => {
    const rec = { kind, message: String(error?.message ?? error), stack: errorInfo?.componentStack ?? '' };
    list.push(rec);
    reactErrors.push(rec); // 统一账本：生产树上的 React 错误未经精确例外即判红
    tripRuntimeErrorGuard(`react ${kind}`, rec.message); // 终态后到达亦不例外
  };
  const root = createRoot(container, {
    onUncaughtError: info(records.uncaught, 'uncaught'),
    onCaughtError: info(records.caught, 'caught'),
    onRecoverableError: info(records.recoverable, 'recoverable'),
    ...options,
  });
  mountedRoots.push({ root, container });
  const act = await getAct();
  try {
    await act(async () => { root.render(element); });
  } catch (e) {
    records.renderError = e;
    if (!captureRenderErrors) {
      /* 非 canary 场景：清理半挂载残骸后再抛出，避免污染后续用例。 */
      try { await unmount({ root, container }); } catch { /* 已崩溃的树卸载失败可忽略 */ }
      throw e;
    }
  }
  return { root, container, records };
}

/** 卸载单个挂载树：登记移除在 finally（即使 root.unmount 抛错也不残留登记，
    错误继续向上抛——不静默）。 */
export async function unmount(entry) {
  try {
    const act = await getAct();
    await act(async () => { entry.root.unmount(); });
  } finally {
    entry.container.remove();
    const idx = mountedRoots.indexOf(entry);
    if (idx >= 0) mountedRoots.splice(idx, 1);
  }
}

/** 在多轮 macrotask 中让 microtask/IPC/effect 链完整推进。 */
export async function flush(rounds = 3, ms = 0) {
  const act = await getAct();
  for (let i = 0; i < rounds; i++) {
    await act(async () => { await new Promise((r) => setTimeout(r, ms)); });
  }
}

/** 轮询直到 predicate 为真（每轮都在 act 内，保证 React 更新被应用）。 */
export async function waitFor(predicate, { label = '条件', timeout = 4000, interval = 15 } = {}) {
  const act = await getAct();
  const start = Date.now();
  for (;;) {
    let ok = false;
    await act(async () => { ok = !!predicate(); await new Promise((r) => setTimeout(r, interval)); });
    if (ok) return;
    if (Date.now() - start > timeout) {
      throw new Error(`waitFor 超时（${timeout}ms）：${label}`);
    }
  }
}

/** 派发真实键盘事件（App 的 window keydown 监听消费；bubbles=true 保证冒泡链完整）。 */
export function pressKey(key, init = {}) {
  const ev = new w.KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...init });
  w.dispatchEvent(ev);
  return ev;
}

/** 卸载全部登记挂载树：逐个继续（一个失败不阻止清理其余），最后把错误聚合成
    AggregateError 抛出——不再 catch{} 吞掉 effect cleanup 错误。 */
export async function unmountAll() {
  const errors = [];
  for (const entry of [...mountedRoots].reverse()) {
    try { await unmount(entry); } catch (e) { errors.push(e); }
  }
  if (errors.length > 0) {
    throw errors.length === 1 ? errors[0] : new AggregateError(errors, `unmountAll：${errors.length} 个挂载树卸载失败（其余已继续清理）`);
  }
}

/* ------------------------------------------------------------------
   8) 状态夹具复位（用例之间隔离）
   ------------------------------------------------------------------ */
const BASELINE_PLAYER = {
  isActive: false, isPlaying: false, speed: 1, title: '', showName: '', cover: '',
  audioUrl: '', positionSec: 0, durationSec: 0, seekToSec: null,
};

/** 复位到「干净但同实例」的基线：只写被测试消费的字段，不动 action 与方法。
    错误账本生命周期由 beginTestErrors/endTestErrors 管理，本函数不再清账本
    （清了会与「每用例统一检查」的指针语义打架）。 */
export function resetStore(extra = {}) {
  useAppStore.setState({
    activeContentLayout: 'article',
    activeViewFilter: 'all',
    activeFeedFilter: 'all',
    timelineFilter: 'unread',
    timelineSort: 'newest',
    activeArticleId: null,
    isShowingTranslatedProse: false,
    isRawRenderMode: false,
    showFulltext: false,
    summarizingIds: {},
    translating: false,
    summaryErrors: {},
    translateErrors: {},
    translatingIds: {},
    rawTranslatedIds: {},
    openedReadIds: {},
    categories: [],
    entries: [],
    feedIndex: new Map(),
    feedCounts: new Map(),
    player: { ...BASELINE_PLAYER },
    playerExpanded: false,
    settingsOpen: false,
    searchOpen: false,
    closeAskVisible: false,
    lightboxUrl: null,
    lightboxEntryId: null,
    newCategoryModalOpen: false,
    addFeedModalOpen: false,
    editFeedModalOpen: false,
    renameCatModalOpen: false,
    toasts: [],
    dataMode: 'mock',
    dataLoading: false,
    bootstrapError: null,
    articlesLimit: 0,
    articlesCursor: {},
    articlesLoading: false,
    articlesExhausted: true,
    positionRestoreNonce: 0,
    switchRestoreNonce: 0,
    readerCloseNonce: 0,
    ...extra,
  });
  resetBodyCacheForTests();
}

/** 全量清理：卸载所有挂载树（聚合错误）、复位 store、移除遗留 DOM、关闭 jsdom。
    任一步失败都在完成其余步骤后聚合抛出（清理必完成、失败非静默）。 */
export async function teardownAll() {
  const errors = [];
  try { await unmountAll(); } catch (e) { errors.push(e); }
  try { resetStore(); } catch (e) { errors.push(e); }
  try {
    const extra = w.document.querySelectorAll('body > div:not(#root)');
    for (const el of extra) el.remove();
    w.document.body.innerHTML = '<div id="root"></div>';
  } catch (e) { errors.push(e); }
  try { dom.window.close(); } catch (e) { errors.push(e); }
  if (errors.length > 0) {
    throw errors.length === 1 ? errors[0] : new AggregateError(errors, `teardownAll：${errors.length} 项清理失败`);
  }
}

/* ------------------------------------------------------------------
   9) 用例包装（R1）：统一「执行 → 卸载清理 → 错误账本终检 → 聚合失败」。
      - 三者任一失败都必须让用例红（含 cleanup 错误与未预期 console/jsdom/react）；
      - 多个失败聚合为 AggregateError（不互相掩盖）。
      旧写法（裸 test + 各自 t.after(unmountAll)）已被本包装取代。
   ------------------------------------------------------------------ */
export function componentTest(name, options, body) {
  const opts = typeof options === 'function' ? {} : (options ?? {});
  const fn = typeof options === 'function' ? options : body;
  return test(name, opts, async (t) => {
    const errors = [];
    const gap = beginTestErrors(); // 间隙错误（上轮结束后新出现）不静默
    if (gap) errors.push(gap);
    try {
      await fn(t);
    } catch (e) {
      errors.push(e);
    }
    try {
      await unmountAll();
    } catch (e) {
      errors.push(e);
    }
    const ledgerErr = endTestErrors();
    if (ledgerErr) errors.push(ledgerErr);
    if (errors.length === 1) throw errors[0];
    if (errors.length > 1) throw new AggregateError(errors, `用例失败/清理/错误账本共 ${errors.length} 项失败`);
  });
}

/* ------------------------------------------------------------------
   10) 生命周期看门狗（ref，R1 修复）：unref 时代，pending 的用例/after 钩子
       让事件循环空转到自然退出（实测 `after(() => new Promise(() => {}))`
       仍 5pass exit0）。改为 ref 定时器主动保活：到期未 disarm 即强制 exit 3。
       正常结束由入口的 runFinalTeardown 显式 disarm。
   ------------------------------------------------------------------ */
let lifecycleTimer = null;

export function armLifecycleWatchdog(ms = Number(process.env.COMPONENT_WATCHDOG_MS ?? 120000)) {
  if (lifecycleTimer) clearTimeout(lifecycleTimer);
  lifecycleTimer = setTimeout(() => {
    origError(`[component-regression] 生命周期看门狗：${ms}ms 内测试未完整结束（用例/after 钩子 pending 或清理未完成），强制非 0 退出`);
    process.exit(3);
  }, ms);
  return lifecycleTimer;
}

export function disarmLifecycleWatchdog() {
  if (lifecycleTimer) {
    clearTimeout(lifecycleTimer);
    lifecycleTimer = null;
  }
}

/** 入口的最终收尾协议：清理必完成、错误账本终检、失败置非 0 退出码后再抛。
    node:test 的 programmatic 模式在 after 钩子失败时**不**改进程退出码（实测
    hook reject 后 exit 0），故这里显式 `process.exitCode = 1`——否则清理失败会
    被静默成绿灯。 */
export async function runFinalTeardown() {
  let failure = null;
  try {
    await teardownAll();
    const late = endTestErrors();
    if (late) failure = late;
  } catch (e) {
    failure = e;
  }
  /* 无论终检成功与否都武装终态守卫：此后任何通道的新运行时错误都立即非 0
     （成功路径靠它兜住 finalcheck 后的晚异步错误——R2 假绿缺口）。 */
  armRuntimeErrorGuard();
  if (failure) {
    origError(`[component-regression] 最终清理/检查失败：${failure?.message ?? failure}`);
    process.exitCode = 1;
    throw failure;
  }
  console.log('[component-regression] 清理完成：挂载树已全部卸载、错误账本干净');
}

/* 供诊断输出：把挂载树数量暴露给入口汇总。 */
export function mountedCount() { return mountedRoots.length; }
