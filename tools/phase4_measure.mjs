// TASK-117（四阶段）/ TASK-128（测量纠正）：实机性能测量——对真实运行的应用做自动化测量电池。
//
// 前置：应用以远程调试端口启动（WebView2）：
//   set WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222
//   FluxReader.exe
// 运行：node tools/phase4_measure.mjs --out tmp/phase4/measure-report.json
// 依赖：tools/t059_cdp.mjs 的连接基建（findPageTarget/connect；该文件受保护，本工具只消费不改）。
// 纪律：本脚本只读测量（点击/滚动/读堆内存/IPC 查询），不写应用数据；DB 注入由 phase4_seed.py 负责。
//
// ---- TASK-128 修正要点（对应审计 P2-7 的六点质疑）----
// 1) 切换延迟：不再用「存在任意卡片」作完成判据。点击前记录卡片 id 基线，点击后以
//    **卡片 id 集合变化**（rAF 16ms 轮询，非 50ms setInterval）为 firstResultMs，
//    再等 **连续 3 帧 rAF 无 DOM 变更**为 stableMs；超时只报 timedOut，不给毫秒数。
// 2) 搜索：分段计时 openMs（Ctrl+K → 浮层打开）与 resultMs（键入 → 结果节点出现）；
//    结果归属断言=「节点必须来自搜索浮层内的 .cp-item」「文本含本次 token」「结果数与基线不同」；
//    超时只报 timedOut，不给 resultMs。另跑一次「确定不在数据里的 token」负向探针，
//    证明命中判定确实只看浮层而不是 document.body。
// 3) 深页滚动：顶部之外再测列表中部 50% 与尾部 90%（先滚到位并等静默 300ms，再采样 3 秒）。
// 4) 内存口径分列：JS 堆（performance.memory）与进程 RSS（app 进程 + WebView2 进程组，
//    Node 侧 PowerShell 读取）；外加长会话 8 轮巡检的堆/RSS 增量。
// 5) 锁等待代理：sync_queue_stats IPC 往返采样，空闲态与同步在飞态各一组（明确标注为代理）。
// 6) 报告自带测量条件（conditions），并在结论行内限定条件；任一测量段缺前置条件时
//    输出 {skipped}/{error}/{timedOut}，绝不给假数字。
import { writeFileSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { execFileSync } from 'node:child_process';
import { connect, findPageTarget, CDP_PORT } from './t059_cdp.mjs';

  const outArg = process.argv.indexOf('--out');
const OUT = outArg > -1 ? resolve(process.argv[outArg + 1]) : resolve('tmp/phase4/measure-report.json');
/** --reload：先刷新页面再测量（重复测量时拿到干净的起点；不写应用数据）。 */
const RELOAD_FIRST = process.argv.includes('--reload');

/** 应用进程名（Tauri productName）与 WebView2 进程组名，可用环境变量覆盖。 */
const APP_PROCESS = process.env.T128_APP_PROCESS || 'FluxReader';
const WEBVIEW_PROCESS = process.env.T128_WEBVIEW_PROCESS || 'msedgewebview2';
/** WebView2 子进程的归属过滤关键字：命令行的 --user-data-dir 里带 Tauri 的 bundle identifier。
    本机可能同时跑着别的 WebView2 应用（本工具实测：系统里另有 47 个 msedgewebview2），
    不加过滤会把它们的内存算到被测应用头上——只报匹配到的，并在 note 里写明过滤口径。 */
const WEBVIEW_OWNER_KEY = process.env.T128_WEBVIEW_OWNER_KEY || 'com.fluxreader.app';

const LAYOUT_NAMES = ['文章', '社交', '画廊', '播客', '通知'];
/** 渲染树被卸载时的统一话术（页面内 health 探针与结论行共用，避免两处说法漂移）。 */
const CRASH_REASON = 'app-render-tree-unmounted（见 report.appHealth）';

/* ============================================================
   页面内测量库（注入一次，后续按名调用）
   注意：这段代码运行在页面里，不写反引号、不写 ${}，避免与外层模板串冲突。
   ============================================================ */
const PAGE_LIB = `
(() => {
  /* 页面错误记录器：先装（不受下面的幂等早退影响）——诊断「应用渲染树崩了导致后续测量段为空」
     这类现象必须有原始证据，不能只写「测不到」。（CDP 事件通道不在 t059_cdp.mjs 契约内，
     故改从页面内挂钩 window.onerror / unhandledrejection / console.error。） */
  if (!window.__t128Errors) {
    window.__t128Errors = [];
    const pushErr = (kind, msg) => {
      if (window.__t128Errors.length >= 60) return;
      window.__t128Errors.push({ atMs: Math.round(performance.now()), kind: kind, msg: String(msg).slice(0, 400) });
    };
    window.addEventListener('error', (e) => pushErr('error', (e && (e.message || (e.error && e.error.message))) || 'unknown'));
    window.addEventListener('unhandledrejection', (e) => pushErr('unhandledrejection', (e && e.reason && (e.reason.message || e.reason)) || 'unknown'));
    const origError = console.error.bind(console);
    console.error = function () {
      const parts = [];
      for (let i = 0; i < arguments.length; i += 1) {
        const a = arguments[i];
        try { parts.push(typeof a === 'string' ? a : (a && a.message) || String(a)); } catch (x) { parts.push('[unserializable]'); }
      }
      pushErr('console.error', parts.join(' '));
      return origError.apply(null, arguments);
    };
  }
  if (window.__t128) return 't128-lib-ready';

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const frame = () => new Promise((r) => requestAnimationFrame((t) => r(t)));
  const r1 = (v) => (typeof v === 'number' && isFinite(v) ? +v.toFixed(1) : null);

  const scroller = () => document.getElementById('timelineContentScroll');
  const timelineRoot = () => scroller() || document.body;

  const pctl = (sorted, q) => {
    if (!sorted.length) return null;
    const i = Math.min(sorted.length - 1, Math.max(0, Math.floor(sorted.length * q)));
    return +sorted[i].toFixed(1);
  };

  /* 卡片身份基线：DOM 里已渲染的 [data-ctx=article][data-id]（文章/社交/画廊/播客/通知五类卡片
     都带这两个属性）。data-card-index 会被复用（同一个 index 换内容），不能当身份。 */
  const cardIds = () => {
    const out = [];
    for (const el of document.querySelectorAll('[data-ctx="article"][data-id]')) out.push(el.getAttribute('data-id'));
    return out;
  };
  const snapshot = () => {
    const ids = cardIds();
    const sc = scroller();
    return { ids: ids, count: ids.length, digest: ids.join('|'), scrollHeight: sc ? Math.round(sc.scrollHeight) : -1 };
  };

  const domNodes = () => document.getElementsByTagName('*').length;
  /* 应用存活探针：React 根被卸载（渲染期异常冒到顶层）时，后续任何测量都只会得到空集——
     必须在报告里显式点名，而不是让读者把「空」当成「快」。 */
  const appHealth = () => {
    const root = document.getElementById('root');
    const sidebar = !!document.querySelector('.sidebar');
    const shell = !!document.querySelector('.timeline-col');
    return {
      alive: !!root && root.childElementCount > 0 && sidebar && shell,
      rootChildren: root ? root.childElementCount : -1,
      hasSidebar: sidebar,
      hasTimeline: shell,
      domNodes: domNodes(),
      cards: document.querySelectorAll('[data-card-index]').length,
      errors: (window.__t128Errors || []).slice(-5),
      errorCount: (window.__t128Errors || []).length,
    };
  };
  const mediaNodes = () => {
    const all = [];
    for (const el of document.querySelectorAll('.timeline-scroll-body img, .timeline-scroll-body audio, .timeline-scroll-body video')) all.push(el.tagName);
    return { count: all.length, imgs: all.filter((t) => t === 'IMG').length, audios: all.filter((t) => t === 'AUDIO').length };
  };
  const heap = () => (window.performance && performance.memory)
    ? { usedJSHeapMB: +(performance.memory.usedJSHeapSize / 1048576).toFixed(2),
        totalJSHeapMB: +(performance.memory.totalJSHeapSize / 1048576).toFixed(2),
        jsHeapLimitMB: +(performance.memory.jsHeapSizeLimit / 1048576).toFixed(2) }
    : null;

  const badgeCount = () => {
    const b = document.querySelector('.feed-all-row .feed-count-badge');
    if (!b) return null;
    const n = parseInt((b.textContent || '').replace(/[^0-9]/g, ''), 10);
    return Number.isNaN(n) ? null : n;
  };
  const syncPill = () => {
    const p = document.querySelector('.sync-status-pill');
    return p ? (p.textContent || '').trim() : null;
  };
  const isTauri = () => typeof window.__TAURI_INTERNALS__ !== 'undefined';
  const activeLayoutName = () => {
    const b = document.querySelector('.nav-tab-item.active-layout');
    return b ? (b.textContent || '').trim() : null;
  };
  const layoutButton = (name) => {
    for (const b of document.querySelectorAll('.nav-tab-item')) {
      if ((b.textContent || '').trim() === name) return b;
    }
    return null;
  };
  const feedRows = () => {
    const out = [];
    for (const el of document.querySelectorAll('[data-ctx="feed"][data-id]')) out.push(el);
    return out;
  };

  const conditions = () => {
    const cards = document.querySelectorAll('[data-card-index]').length;
    const m = mediaNodes();
    const sc = scroller();
    const pill = syncPill();
    /* 角标（feed_counts）在浏览器 mock 下恒为 0（无 IPC）——补一个「虚拟列表总高 / 估算行高」
       的条目数估计，口径写在字段名与 note 里，避免把 0 当成「库里没数据」。 */
    const wrap = document.querySelector('.timeline-virtual-wrap');
    const wrapPx = wrap && wrap.style && wrap.style.height ? parseFloat(wrap.style.height) : NaN;
    return {
      layoutUnderTest: activeLayoutName(),
      articlesLoaded: badgeCount(),
      articlesLoadedSource: 'sidebar .feed-all-row 角标（当前布局口径；非全库总量；mock 无 IPC 时恒为 0）',
      loadedItemsEstimate: Number.isFinite(wrapPx) ? Math.round(wrapPx / 160) : null,
      loadedItemsEstimateNote: '虚拟列表总高 ÷ 估算行高 160px（仅虚拟化布局；动画/未测量行会让它偏大偏小，只作量级参考）',
      cardsVisible: cards,
      mediaNodesInList: m.count,
      mediaEnabled: cards ? m.count > 0 : null,
      network: isTauri() ? 'real' : 'local-mock',
      tauri: isTauri(),
      syncInFlight: !!window.__t128SyncInFlight || /(同步中|等待同步)/.test(pill || ''),
      syncPill: pill,
      domNodes: domNodes(),
      heap: heap(),
      scroller: sc ? { scrollHeight: Math.round(sc.scrollHeight), clientHeight: Math.round(sc.clientHeight), scrollTop: Math.round(sc.scrollTop) } : null,
      appVersion: null,
    };
  };

  const waitUntil = async (fn, timeoutMs, intervalMs) => {
    const step = intervalMs || 16;
    const t0 = performance.now();
    for (;;) {
      let ok = false;
      try { ok = !!fn(); } catch (e) { ok = false; }
      if (ok) return true;
      if (performance.now() - t0 > timeoutMs) return false;
      await sleep(step);
    }
  };

  /* 渲染稳定：MutationObserver 计数 + 连续 frames 帧无变更（可选再叠加 wall-clock 静默时长）。 */
  const waitStable = (opts) => new Promise((resolve) => {
    const o = opts || {};
    const frames = o.frames || 3;
    const quietMs = o.quietMs || 0;
    const timeoutMs = o.timeoutMs || 4000;
    const root = o.root || timelineRoot();
    let pending = 0;
    let lastMutationAt = performance.now();
    const obs = new MutationObserver((recs) => { pending += recs.length; lastMutationAt = performance.now(); });
    try { obs.observe(root, { childList: true, subtree: true, attributes: true, characterData: true }); } catch (e) { /* 观察失败按不稳定处理 */ }
    const t0 = performance.now();
    let rafQuiet = 0;
    let done = false;
    const finish = (timedOut) => {
      if (done) return;
      done = true;
      obs.disconnect();
      resolve({ ms: +(performance.now() - t0).toFixed(1), stableFrames: rafQuiet, timedOut: !!timedOut });
    };
    const tick = () => {
      if (done) return;
      pending += obs.takeRecords().length;
      const now = performance.now();
      if (pending > 0) { pending = 0; rafQuiet = 0; lastMutationAt = now; }
      else rafQuiet += 1;
      const quietOk = rafQuiet >= frames && (!quietMs || now - lastMutationAt >= quietMs);
      if (quietOk) return finish(false);
      if (now - t0 >= timeoutMs) return finish(true);
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });

  /* ---- (a) 切换延迟：新结果识别（id 集合变化）+ 连续 3 帧渲染稳定 ---- */
  const switchLatency = async (target, opts) => {
    const o = opts || {};
    const timeoutMs = o.timeoutMs || 8000;
    const frames = o.frames || 3;
    const el = document.querySelector(target.selector);
    if (!el) return { target: target.label, kind: target.kind, error: 'target-not-found', selector: target.selector };
    const before = snapshot();
    const t0 = performance.now();
    el.click();
    let first = null;
    let after = null;
    let polls = 0;
    while (performance.now() - t0 < timeoutMs) {
      await frame();
      polls += 1;
      const now = snapshot();
      if (now.digest !== before.digest || now.count !== before.count) { first = performance.now() - t0; after = now; break; }
    }
    if (first === null) {
      const end = snapshot();
      return {
        target: target.label, kind: target.kind, timedOut: true, timeoutMs: timeoutMs, polls: polls,
        cardsBefore: before.count, cardsAfter: end.count, changed: false,
        note: '超时内卡片 id 集合未变化：不输出毫秒数（目标可能只是重载同集结果、列表为空或点击未生效）',
      };
    }
    const st = await waitStable({ frames: frames, timeoutMs: 4000 });
    return {
      target: target.label, kind: target.kind, firstResultMs: r1(first), stableMs: st.ms,
      stableFrames: st.stableFrames, stableTimedOut: st.timedOut, polls: polls,
      cardsBefore: before.count, cardsAfter: after.count, changed: true,
      idSampleAfter: after.ids.slice(0, 5),
    };
  };

  /* ---- (c) 深页滚动：先到位，再采样帧间隔 ---- */
  const scrollToRatio = async (ratio, opts) => {
    const o = opts || {};
    const sc = scroller();
    if (!sc) return { error: 'no-timeline-scroller' };
    const range = sc.scrollHeight - sc.clientHeight;
    if (range <= 20) return { skipped: 'list-not-scrollable', scrollHeight: Math.round(sc.scrollHeight), clientHeight: Math.round(sc.clientHeight) };
    sc.scrollTop = Math.round(range * ratio);
    const st = await waitStable({ frames: 1, quietMs: o.quietMs || 300, timeoutMs: o.timeoutMs || 4000 });
    return {
      requestedRatio: ratio, scrollTop: Math.round(sc.scrollTop), scrollHeight: Math.round(sc.scrollHeight),
      clientHeight: Math.round(sc.clientHeight), settleMs: st.ms, settled: !st.timedOut, settleStableFrames: st.stableFrames,
    };
  };

  const frameSample = async (ms, opts) => {
    const o = opts || {};
    const sc = scroller();
    const deltas = [];
    let stop = false;
    let last = performance.now();
    const start = performance.now();
    const tick = (t) => { deltas.push(t - last); last = t; if (!stop && t - start < ms) requestAnimationFrame(tick); };
    requestAnimationFrame(tick);
    let timer = null;
    if (o.scrollStep && sc) timer = setInterval(() => { sc.scrollTop = Math.min(sc.scrollTop + o.scrollStep, sc.scrollHeight); }, 100);
    await sleep(ms + 200);
    stop = true;
    if (timer) clearInterval(timer);
    const sorted = deltas.slice().sort((a, b) => a - b);
    return {
      frames: deltas.length, p50: pctl(sorted, 0.5), p95: pctl(sorted, 0.95),
      max: deltas.length ? +Math.max.apply(null, deltas).toFixed(1) : null,
      jankOver50ms: deltas.filter((d) => d > 50).length,
      scrollHeight: sc ? Math.round(sc.scrollHeight) : null,
      scrollTop: sc ? Math.round(sc.scrollTop) : null,
      scrollRange: sc ? Math.round(sc.scrollHeight - sc.clientHeight) : null,
      sampleMs: Math.round(performance.now() - start),
      scrolled: !!o.scrollStep,
    };
  };

  /* ---- (b) 搜索：分段计时 + 结果归属断言 ---- */
  const setInputValue = (el, value) => {
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
  };
  const pressKey = (key, opts) => {
    const o = opts || {};
    const ev = new KeyboardEvent('keydown', { key: key, code: o.code || key, ctrlKey: !!o.ctrlKey, bubbles: true, cancelable: true });
    window.dispatchEvent(ev);
    return true;
  };
  const searchInput = () => document.querySelector('.search-modal-input');
  const searchOverlay = () => { const i = searchInput(); return i ? i.closest('.modal-overlay') : null; };
  const searchState = () => {
    const i = searchInput();
    if (!i) return { exists: false, open: false, inert: null, focused: null };
    const o = searchOverlay();
    return { exists: true, open: !!(o && o.classList.contains('open')), inert: !!(o && o.hasAttribute('inert')), focused: document.activeElement === i };
  };
  /* 结果节点只在浮层容器内取：这正是审计点 2「完成判据搜整个 document.body」的修法。 */
  const searchItems = () => {
    const o = searchOverlay();
    if (!o) return [];
    const out = [];
    for (const el of o.querySelectorAll('.cp-item')) out.push((el.textContent || '').trim());
    return out;
  };

  const closeSearch = async (timeoutMs) => {
    if (!searchState().open) return true;
    pressKey('Escape', { code: 'Escape' });
    return await waitUntil(() => !searchState().open, timeoutMs || 1500);
  };

  const runSearch = async (params) => {
    const p = params || {};
    const timeoutMs = p.timeoutMs || 6000;
    const openTimeoutMs = p.openTimeoutMs || 3000;
    await closeSearch(1200);
    const t0 = performance.now();
    pressKey('k', { ctrlKey: true, code: 'KeyK' });
    const opened = await waitUntil(() => searchState().open, openTimeoutMs);
    const openMs = performance.now() - t0;
    if (!opened) return { timedOut: true, openMs: r1(openMs), reason: 'search-overlay-not-open', token: p.token };
    const input = searchInput();
    if (!input) return { timedOut: true, openMs: r1(openMs), reason: 'no-search-input', token: p.token };
    const baselineItems = searchItems();
    const baselineCount = baselineItems.length;
    const t1 = performance.now();
    setInputValue(input, p.token);
    let resultMs = null;
    let hit = null;
    let polls = 0;
    while (performance.now() - t1 < timeoutMs) {
      await sleep(16);
      polls += 1;
      const items = searchItems();
      const found = items.filter((t) => t.indexOf(p.token) >= 0);
      if (found.length > 0 && items.length !== baselineCount) { resultMs = performance.now() - t1; hit = found.slice(0, 3); break; }
    }
    const postItems = searchItems();
    const out = {
      token: p.token,
      tokenSource: p.tokenSource || null,
      openMs: r1(openMs),
      baselineResultCount: baselineCount,
      baselineHasToken: baselineItems.some((t) => t.indexOf(p.token) >= 0),
      resultCount: postItems.length,
      resultCountChanged: postItems.length !== baselineCount,
      polls: polls,
      matchedNodes: hit,
      ownership: {
        scope: 'searchOverlay.querySelectorAll(".cp-item")',
        overlayResolved: !!searchOverlay(),
        overlayContainsInput: !!(searchOverlay() && searchOverlay().contains(searchInput())),
        containsToken: !!hit,
        countDiffersFromBaseline: postItems.length !== baselineCount,
      },
    };
    if (resultMs === null) {
      out.timedOut = true;
      out.reason = 'no-result-node-with-token-within-timeout';
      out.note = '超时内未在浮层内找到「含 token 且结果数变化」的结果节点：不输出 resultMs';
    } else {
      out.resultMs = r1(resultMs);
    }
    out.closedAfterEscape = await closeSearch(1500);
    return out;
  };

  /* 触发手动同步：走命令面板的「刷新全部订阅源」（store.triggerManualSync） */
  const triggerManualSyncViaPalette = async () => {
    await closeSearch(1200);
    pressKey('k', { ctrlKey: true, code: 'KeyK' });
    const opened = await waitUntil(() => searchState().open, 2500);
    if (!opened) return { triggered: false, reason: 'palette-not-open' };
    const o = searchOverlay();
    let target = null;
    if (o) {
      for (const el of o.querySelectorAll('.cp-item')) {
        if ((el.textContent || '').indexOf('刷新全部订阅源') >= 0) { target = el; break; }
      }
    }
    if (!target) { await closeSearch(1200); return { triggered: false, reason: 'command-item-not-found' }; }
    target.click();
    window.__t128SyncInFlight = true;
    window.__t128SyncAtMs = performance.now();
    await sleep(60);
    return { triggered: true, pillAfterTrigger: syncPill(), syncInFlightFlag: true };
  };

  /* ---- (f) 锁等待代理：sync_queue_stats IPC 往返 ---- */
  const lockSample = async (n, opts) => {
    const o = opts || {};
    const internals = window.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== 'function') return { skipped: 'not-tauri', note: '浏览器 mock 环境无 IPC，锁等待代理不适用' };
    const samples = [];
    let failure = null;
    for (let i = 0; i < n; i += 1) {
      const t = performance.now();
      try { await internals.invoke('sync_queue_stats'); } catch (e) { failure = String((e && e.message) || e); break; }
      samples.push(performance.now() - t);
      await sleep(o.gapMs === undefined ? 10 : o.gapMs);
    }
    if (!samples.length) return { skipped: 'invoke-failed', error: failure };
    const sorted = samples.slice().sort((a, b) => a - b);
    return {
      n: samples.length, p50: pctl(sorted, 0.5), p95: pctl(sorted, 0.95), p99: pctl(sorted, 0.99),
      min: +sorted[0].toFixed(2), max: +sorted[sorted.length - 1].toFixed(2), unit: 'ms',
      syncInFlight: !!window.__t128SyncInFlight || /(同步中|等待同步)/.test(syncPill() || ''),
      syncPill: syncPill(),
      note: '代理指标：IPC 往返 + sync_queue_stats 单连接查询/锁等待之和；不是直接锁计时',
    };
  };
  /* 对照口径：同一通道上最廉价的 IPC（app 版本），用于说明「纯 IPC 往返」的量级 */
  const ipcBaseline = async (n) => {
    const internals = window.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== 'function') return { skipped: 'not-tauri' };
    const samples = [];
    for (let i = 0; i < n; i += 1) {
      const t = performance.now();
      try { await internals.invoke('plugin:app|version'); } catch (e) { return { skipped: 'invoke-failed', error: String((e && e.message) || e), completed: samples.length }; }
      samples.push(performance.now() - t);
      await sleep(10);
    }
    const sorted = samples.slice().sort((a, b) => a - b);
    return { n: samples.length, p50: pctl(sorted, 0.5), p95: pctl(sorted, 0.95), unit: 'ms', note: '纯 IPC 往返对照（不查库）' };
  };

  /* ---- (d) 长会话巡检：8 轮「切视图 → 等稳定 → 滚一屏」 ---- */
  const longSession = async (rounds, opts) => {
    const o = opts || {};
    const names = o.layouts || ['文章', '社交', '画廊', '播客', '通知'];
    const log = [];
    let crashedAtRound = null;
    for (let i = 0; i < rounds; i += 1) {
      /* 应用渲染树被卸载后继续跑只会产生「找不到按钮」的假记录：如实停下。 */
      const health = window.__t128.appHealth();
      if (!health.alive) { crashedAtRound = i + 1; break; }
      const name = names[i % names.length];
      const rec = { round: i + 1, target: name, switched: false, firstResultMs: null, stableMs: null, timedOut: false };
      const btn = layoutButton(name);
      if (btn) {
        const before = snapshot();
        const t0 = performance.now();
        btn.click();
        while (performance.now() - t0 < 3000) {
          await frame();
          const now = snapshot();
          if (now.digest !== before.digest || now.count !== before.count) { rec.switched = true; rec.firstResultMs = r1(performance.now() - t0); break; }
        }
        if (!rec.switched) rec.timedOut = true;
        const st = await waitStable({ frames: 3, timeoutMs: 3000 });
        rec.stableMs = st.ms;
        rec.stableTimedOut = st.timedOut;
      } else {
        rec.error = 'layout-button-not-found';
      }
      const sc = scroller();
      if (sc && sc.scrollHeight - sc.clientHeight > 20) {
        sc.scrollTop = Math.min(sc.scrollTop + Math.round(sc.clientHeight * 0.8), sc.scrollHeight);
        rec.scroll = { scrollTop: Math.round(sc.scrollTop), scrollHeight: Math.round(sc.scrollHeight) };
      } else {
        rec.scroll = { skipped: 'not-scrollable' };
      }
      await sleep(120);
      log.push(rec);
    }
    return { rounds: rounds, roundsCompleted: log.length, crashedAtRound: crashedAtRound, roundsWithChange: log.filter((x) => x.switched).length, log: log };
  };

  window.__t128 = {
    sleep: sleep, frame: frame, scroller: scroller, snapshot: snapshot, cardIds: cardIds,
    conditions: conditions, waitStable: waitStable, waitUntil: waitUntil, heap: heap, domNodes: domNodes,
    appHealth: appHealth, errors: () => (window.__t128Errors || []).slice(),
    mediaNodes: mediaNodes, layoutButton: layoutButton, layoutNames: () => {
      const out = [];
      for (const b of document.querySelectorAll('.nav-tab-item')) out.push((b.textContent || '').trim());
      return out;
    }, activeLayoutName: activeLayoutName,
    feedRowsInfo: () => feedRows().map((el) => ({ id: el.getAttribute('data-id'), label: (el.textContent || '').trim().replace(/\\s+/g, ' ').slice(0, 40), active: el.className.indexOf('active-feed') >= 0 })),
    switchLatency: switchLatency, scrollToRatio: scrollToRatio, frameSample: frameSample,
    runSearch: runSearch, searchState: searchState, triggerManualSyncViaPalette: triggerManualSyncViaPalette,
    lockSample: lockSample, ipcBaseline: ipcBaseline, longSession: longSession,
    overlayText: () => { const o = searchOverlay(); return o ? (o.textContent || '') : ''; },
    cardTitles: () => {
      const out = [];
      for (const el of document.querySelectorAll('[data-ctx="article"][data-id]')) {
        const t = el.querySelector('.card-title, .social-card-title, .gallery-title, .podcast-title, .notif-title');
        if (t) out.push((t.textContent || '').trim());
      }
      return out;
    },
  };
  return 't128-lib-ready';
})()
`;

/* ============================================================
   Node 侧：进程 RSS（PowerShell）
   ============================================================ */
function readProcessRss() {
  /* 两段查询：
     1) 应用进程（FluxReader.exe）——Rust 侧 + 主窗口；
     2) WebView2 进程组，但只用命令行里带本应用 user-data-dir 关键字（bundle identifier）的那些：
        机器上其它应用（VS Code/Teams/…）也跑 msedgewebview2，全量求和会串味。
     拿不到命令行时退回「按进程名全量求和」，并在 ownerFilter 字段如实标注。 */
  const script = [
    '$a=@(Get-Process -Name \'' + APP_PROCESS + '\' -ErrorAction SilentlyContinue);',
    '$as=($a | Measure-Object WorkingSet64 -Sum).Sum; if ($null -eq $as) { $as = 0 };',
    '$all=@(Get-CimInstance Win32_Process -Filter "Name=\'' + WEBVIEW_PROCESS + '.exe\'" -ErrorAction SilentlyContinue);',
    '$mine=@($all | Where-Object { $_.CommandLine -and $_.CommandLine.Contains(\'' + WEBVIEW_OWNER_KEY + '\') });',
    'function SumWS($procs) { $t=0; foreach ($p in $procs) { $t += $p.WorkingSetSize } ; return $t };',
    '$ms=SumWS $mine; $alls=SumWS $all;',
    '[pscustomobject]@{ appCount=$a.Count; appRssMB=[math]::Round($as/1MB,1);',
    'webviewProcessCount=$mine.Count; webviewRssMB=[math]::Round($ms/1MB,1);',
    'webviewTotalOnMachine=$all.Count; webviewTotalRssMB=[math]::Round($alls/1MB,1) } | ConvertTo-Json -Compress',
  ].join(' ');
  try {
    const out = execFileSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', script], {
      encoding: 'utf8',
      timeout: 30000,
      windowsHide: true,
    });
    const parsed = JSON.parse(String(out).trim());
    const ownerFiltered = parsed.webviewProcessCount > 0;
    return {
      appProcess: APP_PROCESS,
      appProcessCount: parsed.appCount,
      appRssMB: parsed.appProcessCount ? parsed.appRssMB : null,
      webviewProcess: WEBVIEW_PROCESS,
      webviewOwnerFilter: WEBVIEW_OWNER_KEY,
      webviewOwnerFilterMatched: ownerFiltered,
      webviewProcessCount: parsed.webviewProcessCount,
      webviewRssMB: parsed.webviewProcessCount ? parsed.webviewRssMB : null,
      webviewOnMachineTotal: parsed.webviewTotalOnMachine,
      note: 'RSS 含 Rust + WebView2 + 图片/音频解码内存；与 JS 堆口径不同（JS 堆只覆盖渲染进程的 JS 对象）' +
        (ownerFiltered
          ? '；webviewRssMB 只统计命令行含「' + WEBVIEW_OWNER_KEY + '」的 WebView2 进程（排除机器上其它 WebView2 应用）'
          : '；未匹配到本应用的 WebView2 进程（无头浏览器/开发模式下为预期），故 webviewRssMB 为 null；'
            + '机器上同名的 WebView2 进程共有 ' + parsed.webviewTotalOnMachine + ' 个（' + parsed.webviewTotalRssMB + 'MB），不计入'),
      ...(parsed.appProcessCount ? {} : { appRssNote: '未找到应用进程（开发/无头浏览器模式下的预期结果）' }),
    };
  } catch (e) {
    return { error: 'powershell-rss-failed: ' + (e && e.message ? e.message : String(e)), note: 'RSS 读取失败：本轮不提供进程内存数字（不回填、不猜）' };
  }
}

/* ============================================================
   搜索 token 选择：优先取「只在一张卡片标题里出现」的片段，
   这样命中即证明结果属于本次查询（既不是背景列表文本，也不是公共词）。
   ============================================================ */
function chooseSearchToken(titles, overlayText) {
  const scores = new Map();
  const add = (cand) => {
    if (!cand || cand.length < 3) return;
    if (overlayText.includes(cand)) return; // 打开浮层时已可见 → 无法作为「新查询」证据
    scores.set(cand, (scores.get(cand) || 0) + 1);
  };
  for (const raw of titles) {
    const title = String(raw || '').trim();
    if (!title) continue;
    const cjk = title.match(/[\u4e00-\u9fff]{3,}/g) || [];
    for (const run of cjk) {
      for (let i = 0; i + 3 <= run.length; i += 1) add(run.slice(i, i + Math.min(4, run.length - i)));
    }
    const ascii = title.match(/[A-Za-z0-9][A-Za-z0-9._-]{3,}/g) || [];
    for (const w of ascii) add(w.length > 8 ? w.slice(0, 8) : w);
  }
  let best = null;
  for (const [cand, seen] of scores) {
    const key = [seen === 1 ? 0 : 1, -cand.length];
    if (!best || key[0] < best.key[0] || (key[0] === best.key[0] && key[1] < best.key[1])) best = { cand: cand, key: key, seen: seen };
  }
  if (!best) return { token: null, reason: 'no-candidate-token-from-visible-card-titles' };
  return { token: best.cand, seenInTitles: best.seen, source: 'visible-card-title(仅 ' + best.seen + ' 张卡命中)' };
}

/* ============================================================
   主流程
   ============================================================ */
async function main() {
  const report = {
    tool: 'tools/phase4_measure.mjs',
    task: 'TASK-128（审计 P2-7 测量纠正）',
    capturedAt: new Date().toISOString(),
    cdpPort: CDP_PORT,
    node: process.version,
    appProcess: APP_PROCESS,
    webviewProcess: WEBVIEW_PROCESS,
    measures: {},
    conditions: null,
    verdicts: {},
    conclusions: [],
    caveats: [],
  };

  let target;
  try {
    target = await findPageTarget(CDP_PORT);
  } catch (e) {
    report.fatal = '未找到应用页面（确认应用已带 --remote-debugging-port=' + CDP_PORT + ' 启动）：' + e.message;
    writeReport(report);
    console.error(report.fatal);
    process.exit(1);
  }
  const cdp = await connect(target.webSocketDebuggerUrl);
  const send = (method, params = {}) => cdp.send(method, params);
  const evalJs = async (expr) => await cdp.evaluate(expr);

  /** 每个测量段独立 try/catch：任何一段失败都不拖垮整份报告。 */
  const measureOrder = [];
  const section = async (name, fn) => {
    const entry = { name: name, postRecoveryReload: report.appHealth && report.appHealth.recoveries.length > 0 };
    measureOrder.push(entry);
    try {
      const v = await fn();
      report.measures[name] = v;
      entry.ok = !(v && v.error);
      return v;
    } catch (e) {
      report.measures[name] = { error: (e && e.message ? e.message : String(e)) };
      entry.ok = false;
      return report.measures[name];
    }
  };

  const page = (expr) => evalJs(expr);
  const collectGarbage = async () => { try { await send('HeapProfiler.collectGarbage'); } catch { /* 采集失败不阻断 */ } };

  /* 应用存活检查：渲染树被卸载后，后续任何测量都只会得到空集。
     这种时候要显式写下「为什么空」，不能让读者把「空」误读成「快」。
     崩溃后尝试一次页面重载恢复（只重载前端、不写数据），恢复成功的段会在
     measureOrder 里标 postRecoveryReload——重载后堆基线已重置，不可与崩溃前的段混读。 */
  const health = async () => {
    try { return await page('window.__t128.appHealth()'); } catch (e) { return { alive: false, error: e.message }; }
  };
  const crashGuard = async (stepName) => {
    const h = await health();
    if (!h || h.alive) return null;
    let allErrors = null;
    try { allErrors = await page('window.__t128.errors()'); } catch { allErrors = null; }
    const info = { detectedAfter: stepName, at: new Date().toISOString(), health: h, pageErrors: allErrors };
    report.appHealth.alive = false;
    report.appHealth.everCrashed = true;
    report.appHealth.firstDetectedAfter = report.appHealth.firstDetectedAfter || stepName;
    report.appHealth.crashes.push(info);
    report.pageErrors = (report.pageErrors || []).concat(allErrors || []);
    console.error('APP NOT RENDERING after ' + stepName + '：' + JSON.stringify(h.errors || h).slice(0, 300));
    // 自愈尝试：重载页面 + 重新注入测量库；失败就保持 dead，后续段按跳过处理。
    try {
      await send('Page.reload', { ignoreCache: false });
      let ready = false;
      for (let i = 0; i < 40; i += 1) {
        await new Promise((r) => setTimeout(r, 500));
        try { ready = await page("!!document.querySelector('.sidebar') && !!document.querySelector('.timeline-col')"); } catch { ready = false; }
        if (ready) break;
      }
      if (ready) {
        await page(PAGE_LIB);
        const h2 = await page('window.__t128.appHealth()');
        report.appHealth.alive = true;
        report.appHealth.recoveries.push({ after: stepName, at: new Date().toISOString(), healthAfterReload: h2 });
        console.error('页面已重载，后续测量段在「重载后的干净页面」上继续（堆基线已重置）。');
      }
    } catch (e) {
      report.appHealth.recoveryError = e && e.message ? e.message : String(e);
    }
    return info;
  };
  /** 未恢复的崩溃之后，剩余交互段一律跳过（并说明原因），不再制造误导性的空测点。 */
  const crashed = () => report.appHealth.alive === false;

  try {
    if (RELOAD_FIRST) {
      /* 可选：先刷新页面再测量（只重载前端，不写应用数据）。重复测量时保证同一基线；
         刷新后前端会从本机 DB 重新装载状态。 */
      try { await send('Page.enable'); } catch { /* Page 域不可用时直接 reload */ }
      await send('Page.reload', { ignoreCache: false });
      let ready = false;
      for (let i = 0; i < 60; i += 1) {
        await new Promise((r) => setTimeout(r, 500));
        try {
          ready = await page("!!document.querySelector('.sidebar') && !!document.querySelector('.timeline-col')");
        } catch { ready = false; }
        if (ready) break;
      }
      if (!ready) console.error('--reload：刷新后 30 秒内未见应用外壳（继续按可测段测量）');
    }

    const lib = await page(PAGE_LIB);
    if (lib !== 't128-lib-ready') throw new Error('页面内测量库注入失败：' + JSON.stringify(lib));

    // 条件快照（含 app 版本，读不到就 null——不猜）
    await section('appVersionProbe', async () => {
      try {
        const v = await page("(async () => { try { return await window.__TAURI_INTERNALS__.invoke('plugin:app|version'); } catch (e) { return null; } })()");
        return { appVersion: typeof v === 'string' ? v : null, source: 'plugin:app|version' };
      } catch (e) {
        return { appVersion: null, error: e.message };
      }
    });
    const conditionsStart = await page('window.__t128.conditions()');
    if (report.measures.appVersionProbe && report.measures.appVersionProbe.appVersion) {
      conditionsStart.appVersion = report.measures.appVersionProbe.appVersion;
    }
    report.conditions = conditionsStart;
    report.appHealth = { alive: true, everCrashed: false, crashes: [], recoveries: [], initialCheck: await health() };
    report.processRssStart = readProcessRss();
    /* ---------------------------------------------------------------
       测量段顺序：先做对「单布局/任意布局」都成立的交互段，最后才做
       需要遍历五种布局的段（遍历会碰到应用自身的渲染缺陷——如浏览器 mock
       下通知布局会让 React 渲染树崩掉；真机 Tauri 数据下未必复现）。
       这样即使在遍历时崩了，前面的段也已经有有效数据。
       --------------------------------------------------------------- */

    /* ---- (a) 切换延迟：侧栏订阅源行（feed）+ 内容布局按钮（layout）---- */
    if (!crashed()) await section('switchLatency', async () => {
      const feeds = await page('window.__t128.feedRowsInfo()');
      const targets = [];
      for (const f of (feeds || []).filter((x) => !x.active).slice(0, 3)) {
        targets.push({ kind: 'feed', label: f.label, selector: '[data-ctx="feed"][data-id="' + f.id + '"]' });
      }
      for (const name of ['社交', '文章']) {
        targets.push({ kind: 'layout', label: '布局:' + name, selector: '__layout__:' + name });
      }
      const out = [];
      for (const t of targets) {
        let res;
        if (t.selector.startsWith('__layout__:')) {
          /* 布局按钮没有稳定选择器（按文案渲染），临时打标记再交回页面内测点。 */
          const name = t.selector.slice('__layout__:'.length);
          res = await page('(async () => { const b = window.__t128.layoutButton(' + JSON.stringify(name) + '); if (!b) return { target: ' + JSON.stringify(t.label) + ', kind: "layout", error: "layout-button-not-found" }; b.setAttribute("data-t128-target", "1"); const r = await window.__t128.switchLatency({ kind: "layout", label: ' + JSON.stringify(t.label) + ', selector: "[data-t128-target]" }, {}); b.removeAttribute("data-t128-target"); return r; })()');
        } else {
          res = await page('window.__t128.switchLatency(' + JSON.stringify(t) + ', {})');
        }
        out.push(res);
        await page('window.__t128.waitStable({ frames: 3, timeoutMs: 2500 })');
        await crashGuard('switchLatency:' + t.label);
      }
      if (!out.length) return { skipped: 'no-switch-target', note: '侧栏没有可见的订阅源行且布局按钮不可用（分类可能处于折叠态）' };
      return out;
    });

    /* ---- (b) 搜索：负向探针（确定不存在的 token）+ 正向计时（取自当前可见卡片标题）---- */
    if (!crashed()) await section('search', async () => {
      const overlayText = (await page('window.__t128.overlayText()')) || '';
      const negative = await page('window.__t128.runSearch({ token: ' + JSON.stringify('t128absent' + Math.random().toString(36).slice(2, 10)) + ', tokenSource: "随机合成（确定不在数据中）", timeoutMs: 2500 })');
      const titles = (await page('window.__t128.cardTitles()')) || [];
      const picked = chooseSearchToken(titles, overlayText);
      let positive = null;
      if (!picked.token) {
        positive = { skipped: picked.reason, note: '当前布局没有可见卡片标题可用于取 token；正向计时未执行（不编造）' };
      } else {
        positive = await page('window.__t128.runSearch(' + JSON.stringify({ token: picked.token, tokenSource: picked.source, timeoutMs: 6000 }) + ')');
      }
      await crashGuard('search');
      return {
        negativeProbe: negative,
        positive: positive,
        tokenCandidatesFromTitles: titles.length,
        note: 'openMs=Ctrl+K→浮层打开；resultMs=键入→浮层内出现含 token 且结果数变化的结果节点；两者都只在成立时输出数字',
      };
    });

    /* ---- (c) 深页滚动：顶部 / 中部 50% / 尾部 90% 各采样 3 秒 ---- */
    if (!crashed()) await section('scroll', async () => {
      const positions = [['top', 0], ['mid', 0.5], ['bottom', 0.9]];
      const results = [];
      for (const [position, ratio] of positions) {
        const pre = await page('window.__t128.conditions()');
        const settled = await page('window.__t128.scrollToRatio(' + ratio + ', { quietMs: 300, timeoutMs: 4000 })');
        if (settled && (settled.skipped || settled.error)) {
          results.push({ position: position, ...settled, conditions: pre });
          continue;
        }
        const stats = await page('window.__t128.frameSample(3000, { scrollStep: 240 })');
        results.push({
          position: position,
          ratio: ratio,
          ...stats,
          settle: settled,
          conditions: pre,
        });
      }
      const layoutNow = results.length && results[0].conditions ? results[0].conditions.layoutUnderTest : null;
      await crashGuard('scroll');
      return {
        layout: layoutNow,
        note: '每个位置先滚到位并等虚拟列表静默 300ms，再采集 3 秒 rAF 帧间隔（期间每 100ms 程序化推进 240px，与旧版同一手法但位置不同）',
        positions: results,
      };
    });

    /* ---- (f) 锁等待代理：空闲态 + 同步在飞态 ---- */
    if (!crashed()) await section('lockWaitProxy', async () => {
      const idle = await page('window.__t128.lockSample(20, {})');
      const ipc = await page('window.__t128.ipcBaseline(10)');
      if (idle && idle.skipped) {
        return { idle: idle, ipcBaseline: ipc, syncInFlight: { skipped: idle.skipped }, note: '无 Tauri IPC：锁等待代理不适用（如实跳过）' };
      }
      const trigger = await page('window.__t128.triggerManualSyncViaPalette()');
      const inFlight = await page('window.__t128.lockSample(20, {})');
      await crashGuard('lockWaitProxy');
      return {
        idle: idle,
        ipcBaseline: ipc,
        syncInFlight: inFlight,
        trigger: trigger,
        note: '空闲态与同步在飞态各采样 20 次 sync_queue_stats IPC 往返；同步由命令面板「刷新全部订阅源」触发',
      };
    });

    /* ---- (d) 长会话：8 轮视图往返 + 反复深页滚动，报告首末增量的堆/RSS ---- */
    if (!crashed()) await section('longSession', async () => {
      await collectGarbage();
      const before = await page('window.__t128.conditions()');
      const rssBefore = readProcessRss();
      const session = await page('window.__t128.longSession(8, { layouts: ["文章", "社交", "画廊", "播客", "通知"] })');
      await collectGarbage();
      const after = await page('window.__t128.conditions()');
      const rssAfter = readProcessRss();
      const heapStart = before.heap ? before.heap.usedJSHeapMB : null;
      const heapEnd = after.heap ? after.heap.usedJSHeapMB : null;
      const afterHealth = await crashGuard('longSession');
      /* 巡检被打断（崩了 / 找不到按钮）时，首末堆值不在同一会话上：增量无意义，必须作废。 */
      const incomplete = !!afterHealth || (session && (session.crashedAtRound || session.roundsCompleted < session.rounds));
      const growth = heapStart !== null && heapEnd !== null ? +(heapEnd - heapStart).toFixed(2) : null;
      return {
        rounds: 8,
        roundsCompleted: session ? session.roundsCompleted : 0,
        crashedAtRound: session ? session.crashedAtRound : null,
        jsHeapStartMB: heapStart,
        jsHeapEndMB: incomplete ? null : heapEnd,
        growthMB: incomplete ? null : growth,
        growthRawMB: growth,
        growthValid: !incomplete,
        growthInvalidReason: incomplete
          ? ('巡检在第 ' + ((session && session.crashedAtRound) || '?') + ' 轮中断（应用渲染树被卸载或布局按钮缺失）：首末堆值不在同一会话上，增量作废')
          : null,
        domNodesStart: before.domNodes,
        domNodes: after.domNodes,
        domNodesDelta: after.domNodes - before.domNodes,
        rssStart: rssBefore,
        rssEnd: rssAfter,
        appRssDeltaMB: rssBefore.appRssMB !== null && rssBefore.appRssMB !== undefined && rssAfter.appRssMB !== null && rssAfter.appRssMB !== undefined ? +(rssAfter.appRssMB - rssBefore.appRssMB).toFixed(1) : null,
        webviewRssDeltaMB: rssBefore.webviewRssMB !== null && rssBefore.webviewRssMB !== undefined && rssAfter.webviewRssMB !== null && rssAfter.webviewRssMB !== undefined ? +(rssAfter.webviewRssMB - rssBefore.webviewRssMB).toFixed(1) : null,
        log: session ? session.log : null,
        roundsWithChange: session ? session.roundsWithChange : null,
        conditionsBefore: before,
        conditionsAfter: after,
        appCrashedDuringSession: !!afterHealth,
        note: '读堆前先 HeapProfiler.collectGarbage；JS 堆与进程 RSS 口径不同，两列不可互相换算；深页滚动由每轮 scrollTop 推进 0.8 屏实现',
      };
    });

    /* ---- (e) 各布局内存/卡片/媒体（遍历五布局，放最后）---- */
    if (!crashed()) await section('memoryByLayout', async () => {
      const out = [];
      for (const name of LAYOUT_NAMES) {
        const clicked = await page('(() => { const b = window.__t128.layoutButton(' + JSON.stringify(name) + '); if (!b) return false; b.click(); return true; })()');
        if (!clicked) { out.push({ layout: name, skipped: 'layout-button-not-found' }); continue; }
        await page('window.__t128.waitStable({ frames: 3, timeoutMs: 3000 })');
        await collectGarbage();
        const cond = await page('window.__t128.conditions()');
        const media = await page('window.__t128.mediaNodes()');
        out.push({
          layout: name,
          heapUsedMB: cond.heap ? cond.heap.usedJSHeapMB : null,
          heap: cond.heap,
          cards: cond.cardsVisible,
          mediaNodes: media ? media.count : null,
          mediaEnabled: cond.mediaEnabled,
          articlesLoaded: cond.articlesLoaded,
          rss: readProcessRss(),
        });
        const h = await crashGuard('memoryByLayout:' + name);
        if (h) {
          out[out.length - 1].appCrashedHere = true;
          out[out.length - 1].healthAfter = h.health;
          break;
        }
      }
      return out;
    });

    report.processRssEnd = readProcessRss();
    report.conditionsEnd = await page('window.__t128.conditions()');
    report.pageErrors = await page('window.__t128.errors()');
    report.appHealthFinal = await health();
    report.measureOrder = measureOrder;
    /* 崩溃发生在测量段之间的兜底检查：所有段都跑完后再确认一次。 */
    await crashGuard('battery-end');

    buildVerdicts(report);
    writeReport(report);
    printSummary(report);
    cdp.close();
    process.exit(0);
  } catch (e) {
    report.fatal = e && e.message ? e.message : String(e);
    buildVerdicts(report);
    writeReport(report);
    console.error('MEASURE FAILED:', report.fatal);
    try { cdp.close(); } catch { /* 已断开 */ }
    process.exit(1);
  }
}

function writeReport(report) {
  mkdirSync(dirname(OUT), { recursive: true });
  writeFileSync(OUT, JSON.stringify(report, null, 1));
  console.log('report written:', OUT);
}

/** 结论只允许在受测条件下陈述；每项都带判定线与本测点是否越线。 */
function buildVerdicts(report) {
  const c = report.conditions || {};
  const condLine =
    '受测条件：布局=' + String(c.layoutUnderTest) +
    '、已加载实体≈' + String(c.articlesLoaded) + (c.loadedItemsEstimate != null ? '（虚拟列表估算 ' + c.loadedItemsEstimate + ' 条）' : '') +
    '、可见卡片=' + String(c.cardsVisible) +
    '、媒体=' + String(c.mediaEnabled) +
    '、网络=' + String(c.network) +
    '、同步在飞=' + String(c.syncInFlight) +
    (c.appVersion ? '、版本=' + c.appVersion : '');
  const conclusions = [];

  // 切换
  const sw = report.measures.switchLatency;
  if (Array.isArray(sw) && sw.length) {
    const ok = sw.filter((x) => !x.timedOut && !x.error && typeof x.firstResultMs === 'number');
    const crossed = ok.filter((x) => x.firstResultMs > 1000);
    report.verdicts.switchLatency = {
      decisionLineMs: 1000,
      measured: ok.length,
      crossed: crossed.map((x) => x.target),
      timedOut: sw.filter((x) => x.timedOut).map((x) => x.target),
      skipped: sw.filter((x) => x.skipped).map((x) => x.skipped),
      field: 'firstResultMs（新结果落地）与 stableMs（连续 3 帧无 DOM 变更）分列',
    };
    conclusions.push(
      ok.length
        ? condLine + '：切换 ' + ok.length + ' 个目标的最慢 firstResultMs=' + Math.max(...ok.map((x) => x.firstResultMs)) +
          'ms、stableMs 最大=' + Math.max(...ok.map((x) => x.stableMs || 0)) + 'ms（判定线 >1000ms）→ ' +
          (crossed.length ? '有目标越线：' + crossed.map((x) => x.target).join('/') : '未观察到越线')
        : condLine + '：切换延迟无有效测点（全部超时/跳过）→ 不支持任何切换性能结论',
    );
  } else {
    report.verdicts.switchLatency = { decisionLineMs: 1000, measured: 0, error: sw && sw.error ? sw.error : 'no-data' };
    conclusions.push(condLine + '：切换延迟无测点 → 无结论');
  }

  // 搜索
  const s = report.measures.search;
  if (s && s.positive && typeof s.positive.resultMs === 'number') {
    const total = (s.positive.openMs || 0) + s.positive.resultMs;
    report.verdicts.search = {
      decisionLineMs: 1000,
      openMs: s.positive.openMs,
      resultMs: s.positive.resultMs,
      openPlusResultMs: +total.toFixed(1),
      crossed: total > 1000 || s.positive.resultMs > 1000,
      ownership: s.positive.ownership,
    };
    conclusions.push(
      condLine + '：搜索 openMs=' + s.positive.openMs + '、resultMs=' + s.positive.resultMs +
      '（合计 ' + total.toFixed(1) + 'ms，判定线 >1000ms）→ ' +
      (total > 1000 ? '越线' : '未观察到越线') +
      '；结果归属：' + JSON.stringify(s.positive.ownership),
    );
  } else if (s && s.positive && s.positive.timedOut) {
    report.verdicts.search = { decisionLineMs: 1000, timedOut: true, reason: s.positive.reason, openMs: s.positive.openMs };
    conclusions.push(condLine + '：搜索 resultMs 无有效测点（timedOut=' + s.positive.reason + '，openMs=' + s.positive.openMs + '）→ 不支持搜索性能结论');
  } else {
    report.verdicts.search = { decisionLineMs: 1000, error: (s && (s.error || (s.positive && s.positive.skipped))) || 'no-data' };
    conclusions.push(condLine + '：搜索无测点 → 无结论');
  }
  if (s && s.negativeProbe) {
    const neg = s.negativeProbe;
    report.verdicts.searchNegativeProbe = {
      token: neg.token,
      matched: !!neg.ownership && neg.ownership.containsToken,
      timedOut: !!neg.timedOut,
      verdict: neg.timedOut ? 'PASS：不存在于数据的 token 未产生「命中」' : 'FAIL：无数据 token 也判为命中，说明判据并非新查询结果',
    };
  }

  // 滚动
  const sc = report.measures.scroll;
  if (sc && Array.isArray(sc.positions)) {
    const pos = sc.positions.filter((p) => typeof p.p95 === 'number');
    report.verdicts.scroll = {
      decisionLineMs: 50,
      perPosition: pos.map((p) => ({ position: p.position, p50: p.p50, p95: p.p95, max: p.max, frames: p.frames, scrollTop: p.scrollTop })),
      crossed: pos.filter((p) => p.p95 > 50).map((p) => p.position),
      skipped: sc.positions.filter((p) => p.skipped || p.error).map((p) => ({ position: p.position, why: p.skipped || p.error })),
    };
    conclusions.push(
      pos.length
        ? condLine + '：滚动帧间隔 p95（判定线 >50ms）→ ' + pos.map((p) => p.position + '=' + p.p95 + 'ms').join('、') + '：' +
          (pos.some((p) => p.p95 > 50) ? '有位置越线' : '未观察到越线') +
          '（三档规模/一档规模的差别见 --info 与 conditions；单次运行不代表其他数据规模）'
        : condLine + '：滚动无有效测点（列表不可滚动）→ 无结论',
    );
  } else {
    report.verdicts.scroll = { decisionLineMs: 50, error: (sc && sc.error) || 'no-data' };
    conclusions.push(condLine + '：滚动无测点 → 无结论');
  }

  // 内存
  const mem = report.measures.memoryByLayout;
  const sess = report.measures.longSession;
  if (Array.isArray(mem) || (sess && !sess.error)) {
    report.verdicts.memory = {
      decisionLine: '内存随文章数超线性增长；长会话缓存应有界',
      jsHeapByLayoutMB: Array.isArray(mem) ? mem.map((m) => ({ layout: m.layout, heapUsedMB: m.heapUsedMB, cards: m.cards, appCrashedHere: !!m.appCrashedHere })) : null,
      longSession: sess && !sess.error ? { rounds: sess.rounds, roundsCompleted: sess.roundsCompleted, crashedAtRound: sess.crashedAtRound, jsHeapStartMB: sess.jsHeapStartMB, jsHeapEndMB: sess.jsHeapEndMB, growthMB: sess.growthMB, growthValid: sess.growthValid, growthInvalidReason: sess.growthInvalidReason, appRssDeltaMB: sess.appRssDeltaMB, webviewRssDeltaMB: sess.webviewRssDeltaMB, domNodesDelta: sess.domNodesDelta, appCrashedDuringSession: !!sess.appCrashedDuringSession } : { error: (sess && sess.error) || 'no-data' },
      singleScalePoint: true,
    };
    const growth = sess && !sess.error && sess.growthValid ? sess.growthMB : null;
    conclusions.push(
      condLine + '：内存 JS 堆=' + (Array.isArray(mem) ? mem.map((m) => m.layout + ':' + m.heapUsedMB + 'MB×' + m.cards + '卡').join('、') : 'n/a') +
        (growth !== null && growth !== undefined
          ? '；长会话 8 轮 JS 堆增量=' + growth + 'MB（进程 RSS 增量见 longSession，口径不同不可换算）'
          : '；长会话增量作废（' + ((sess && (sess.growthInvalidReason || sess.error)) || 'no-data') + '）——不输出可能被误读的增量数字') +
        '；单档数据规模无法判定「超线性」——需两档规模（如 20k/50k）同条件各跑一次再比较',
    );
  }

  // 锁等待代理
  const lk = report.measures.lockWaitProxy;
  if (lk) {
    if (lk.idle && lk.idle.skipped) {
      report.verdicts.lockWaitProxy = { skipped: lk.idle.skipped, note: lk.note };
      conclusions.push(condLine + '：锁等待代理跳过（' + lk.idle.skipped + '）→ 无锁等待结论');
    } else {
      report.verdicts.lockWaitProxy = {
        proxy: true,
        decisionLineMs: 50,
        idle: lk.idle, syncInFlight: lk.syncInFlight, ipcBaseline: lk.ipcBaseline,
        crossed: [lk.idle, lk.syncInFlight].filter((x) => x && typeof x.p95 === 'number' && x.p95 > 50).length > 0,
      };
      conclusions.push(
        condLine + '：锁等待代理（IPC 往返 + 单连接查询/锁等待，非直接锁计时）空闲 p50/p95/p99=' +
        [lk.idle && lk.idle.p50, lk.idle && lk.idle.p95, lk.idle && lk.idle.p99].join('/') +
        'ms、同步在飞=' + [lk.syncInFlight && lk.syncInFlight.p50, lk.syncInFlight && lk.syncInFlight.p95, lk.syncInFlight && lk.syncInFlight.p99].join('/') +
        'ms（代理判定线 p95>50ms）→ ' + (report.verdicts.lockWaitProxy.crossed ? '有越线，建议用 Rust 侧计时复核' : '未观察到越线'),
      );
    }
  }

  report.conclusions = conclusions;
  report.caveats = [
    'JS 堆（performance.memory）不含 Rust 侧、WebView2 与图片/音频解码内存；进程 RSS 含全部但不可与 JS 堆换算。',
    '滚动测量是程序化滚动 + rAF 帧间隔，不代表真实输入延迟；每位置仅 3 秒采样。',
    '锁等待是 IPC 往返代理，不是数据库锁计时；要区分二者需要 Rust 侧埋点。',
    '单次运行只覆盖一个数据规模与一种网络状态，结论不得外推到其他规模/网络/布局组合。',
    '搜索/切换的 resultMs/firstResultMs 为页面内 performance.now() 口径，不含 Node/CDP 往返开销。',
  ];
  if (report.appHealth && report.appHealth.alive === false) {
    report.caveats.push(
      '本次运行中应用渲染树被卸载且未能自动恢复（首次出现在 ' + report.appHealth.firstDetectedAfter +
      ' 之后）：之后的交互段全部按 ' + CRASH_REASON + ' 跳过——本报告只能用于「哪一段还能测」，不能作为整体性能结论。',
    );
    conclusions.push('⚠ 应用渲染树中途被卸载，剩余测量段已跳过（原因见 appHealth / pageErrors）；本报告不完整，勿据此下性能结论。');
  } else if (report.appHealth && report.appHealth.everCrashed) {
    report.caveats.push(
      '本次运行中应用渲染树曾在中途被卸载（首次出现在 ' + report.appHealth.firstDetectedAfter +
      ' 之后），工具随后重载页面恢复测量；崩溃前的段与重载后的段堆基线不同（见 measureOrder[*].postRecoveryReload），不可混读。',
    );
    conclusions.push('⚠ 测量过程中应用渲染树曾崩一次（已记录在 appHealth.crashes 与 pageErrors），测量在重载后的干净页面上继续。');
  }
}

function printSummary(report) {
  console.log(JSON.stringify({ conditions: report.conditions, appHealth: report.appHealth, verdicts: report.verdicts }, null, 1));
  console.log('---- 结论（限定在受测条件下）----');
  for (const line of report.conclusions) console.log('- ' + line);
  if (report.fatal) console.log('FATAL: ' + report.fatal);
}

main().catch((e) => {
  console.error('MEASURE FAILED:', e && e.message ? e.message : e);
  process.exit(1);
});
