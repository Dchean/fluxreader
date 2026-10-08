
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

  /* ---- (b) 搜索：分段计时 + 结果归属断言 ----
     浮层结构（src/components/Overlays.tsx）：操作 / 订阅源 / 文章 三组，每组都是
     <div role="group" aria-label={title}>，组内条目一律 .cp-item。所以「命中」必须落在**文章组**
     里：命令与订阅源条目同样带 .cp-item，订阅源组还会按查询词过滤——token 只要是某个订阅源名的
     子串（如 'Verge' ⊂ 'The Verge'），不限定分组就会把「订阅源过滤完成」（180ms 防抖量级）
     当成「FTS 搜到文章」的 resultMs（审计 R1-F1）。 */
  const ARTICLE_GROUP_LABEL = '文章';
  const NON_ARTICLE_GROUP_LABELS = ['操作', '订阅源'];
  const paletteGroups = () => {
    const o = searchOverlay();
    const out = [];
    if (!o) return out;
    for (const g of o.querySelectorAll('[role="group"]')) out.push({ el: g, label: g.getAttribute('aria-label') });
    return out;
  };
  /* 文章分组解析：优先 [aria-label=文章]；标签查不到时回退到最后一个 role=group，
     但该回退组若本身是「操作/订阅源」，就判为不可用（宁可超时也不接受订阅源组的命中）。 */
  const articleGroupScope = () => {
    const groups = paletteGroups();
    if (!groups.length) return null;
    let g = null;
    let resolvedBy = null;
    for (const x of groups) {
      if (x.label === ARTICLE_GROUP_LABEL) { g = x; resolvedBy = 'aria-label'; break; }
    }
    if (!g) { g = groups[groups.length - 1]; resolvedBy = 'last-group-fallback'; }
    const usable = resolvedBy === 'aria-label' ||
      (resolvedBy === 'last-group-fallback' && NON_ARTICLE_GROUP_LABELS.indexOf(String(g.label)) < 0);
    return { el: g.el, label: g.label, resolvedBy: resolvedBy, usable: usable };
  };
  const articleGroupItems = () => {
    const g = articleGroupScope();
    if (!g || !g.usable) return [];
    const out = [];
    for (const el of g.el.querySelectorAll('.cp-item')) out.push((el.textContent || '').trim());
    return out;
  };
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

  /* 键入前基线采样：打开浮层 → 读整层条目文本 + 分组标签 + 文章组条目 → 关掉。
     用途是给 Node 侧选 token 提供「浮层已打开时可见的文本」（旧版在浮层从未打开时读 overlayText，
     恒为空串，排除守卫永不生效——审计 R1-F1）。 */
  const paletteBaseline = async () => {
    await closeSearch(1200);
    pressKey('k', { ctrlKey: true, code: 'KeyK' });
    const opened = await waitUntil(() => searchState().open, 2500);
    if (!opened) return { opened: false, items: [], groups: [], text: '', itemCount: 0, articleGroupCount: null };
    const items = searchItems();
    const groups = paletteGroups().map((g) => g.label);
    const scope = articleGroupScope();
    const articleItems = articleGroupItems();
    const out = {
      opened: true, items: items, groups: groups, text: items.join('\n'), itemCount: items.length,
      articleGroupCount: articleItems.length,
      articleGroupLabel: scope ? scope.label : null,
      articleGroupResolvedBy: scope ? scope.resolvedBy : null,
    };
    await closeSearch(1200);
    return out;
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
    /* 键入前基线：全部在浮层**打开之后**读。
       - 整层条目文本 → 硬断言本次 token 不在其中（否则「命中」可能只是既有条目，不是新查询结果）
       - 文章组条目数 → 「结果数变化」只对文章组计 */
    const baselineItems = searchItems();
    const baselineCount = baselineItems.length;
    const baselineGroups = paletteGroups().map((g) => g.label);
    const baselineScope = articleGroupScope();
    const baselineArticleItems = articleGroupItems();
    const baselineArticleCount = baselineArticleItems.length;
    const tokenAlreadyPresent = baselineItems.some((t) => t.indexOf(p.token) >= 0);
    const scopeInfo = {
      scope: '搜索浮层内 [role="group"][aria-label="文章"] 下的 .cp-item（标签查不到时回退到最后一个 [role="group"]，该组不得是「操作/订阅源」）',
      overlayResolved: !!searchOverlay(),
      overlayContainsInput: !!(searchOverlay() && searchOverlay().contains(searchInput())),
      paletteGroupsAtBaseline: baselineGroups,
      articleGroupResolved: !!baselineScope,
      articleGroupResolvedBy: baselineScope ? baselineScope.resolvedBy : null,
      articleGroupLabel: baselineScope ? baselineScope.label : null,
      articleGroupUsable: !!(baselineScope && baselineScope.usable),
      baselinePaletteItemCount: baselineCount,
      baselineArticleGroupCount: baselineArticleCount,
      tokenAbsentBeforeTyping: !tokenAlreadyPresent,
    };
    const out = {
      token: p.token,
      tokenSource: p.tokenSource || null,
      openMs: r1(openMs),
      baselineResultCount: baselineCount,
      baselineArticleGroupCount: baselineArticleCount,
      baselineHasToken: tokenAlreadyPresent,
      polls: 0,
      matchedNodes: null,
      ownership: scopeInfo,
    };
    if (tokenAlreadyPresent) {
      /* 硬断言失败：token 在键入前就已出现在浮层条目里（命令/订阅源分组也有 .cp-item），
         此时任何「命中」都无法证明来自本次新查询 —— 作废本段，不输出 resultMs。 */
      out.invalid = true;
      out.reason = 'token-present-in-palette-before-typing';
      out.note = '键入前浮层条目里已含该 token：命中不能证明是本次查询结果，本段不给 resultMs（请换 token）';
      out.closedAfterEscape = await closeSearch(1500);
      return out;
    }
    const t1 = performance.now();
    setInputValue(input, p.token);
    let resultMs = null;
    let hit = null;
    let polls = 0;
    let hitScope = null;
    while (performance.now() - t1 < timeoutMs) {
      await sleep(16);
      polls += 1;
      const items = articleGroupItems(); /* 只有「文章分组」的条目算数 */
      const found = items.filter((t) => t.indexOf(p.token) >= 0);
      if (found.length > 0 && items.length !== baselineArticleCount) {
        resultMs = performance.now() - t1;
        hit = found.slice(0, 3);
        hitScope = articleGroupScope();
        break;
      }
    }
    const finalScope = hitScope || articleGroupScope();
    const postArticleItems = articleGroupItems();
    const postItems = searchItems();
    out.polls = polls;
    out.matchedNodes = hit;
    out.resultCount = postItems.length;
    out.resultCountChanged = postItems.length !== baselineCount;
    out.articleGroupResultCount = postArticleItems.length;
    out.articleGroupCountChanged = postArticleItems.length !== baselineArticleCount;
    out.ownership = {
      ...scopeInfo,
      articleGroupResolvedAtEnd: !!finalScope,
      articleGroupResultCount: postArticleItems.length,
      articleGroupCountChanged: postArticleItems.length !== baselineArticleCount,
      containsToken: !!hit,
      hitGroupLabel: hit && finalScope ? finalScope.label : null,
      hitGroupResolvedBy: hit && finalScope ? finalScope.resolvedBy : null,
      hitInsideArticleGroup: !!(hit && finalScope && finalScope.usable),
    };
    if (resultMs === null) {
      out.timedOut = true;
      out.reason = 'no-article-group-item-with-token-within-timeout';
      out.note = '超时内文章分组（[aria-label=文章]）未出现「含 token 且结果数变化」的条目：不输出 resultMs' +
        (baselineScope && !baselineScope.usable ? '（浮层里没有可用的文章分组，回退组是「' + String(baselineScope.label) + '」）' : '');
    } else {
      out.resultMs = r1(resultMs);
    }
    out.closedAfterEscape = await closeSearch(1500);
    return out;
  };

  /* 触发手动同步：走命令面板的「刷新全部订阅源」（store.triggerManualSync）。
     ！！本工具唯一的真实写操作！！会联网抓取全部订阅源、写本机 DB、可能向后端推送状态
     （审计 R1-F4）。--no-sync-probe 时 Node 侧不会调用本函数。 */
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
      /* 逐轮堆采样（R1-F5）：只采首末两点撑不起「多轮后仍单调增长」的判定线，这里补上每轮读数。
         口径：逐轮读数**不**强制 GC，只用于看趋势；首末增量另见 longSession.growthMB（首末各强制 GC）。 */
      const h = heap();
      rec.jsHeapMB = h ? h.usedJSHeapMB : null;
      rec.domNodes = domNodes();
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
    /* R1-F1：token 选择用的浮层文本必须在**浮层打开后**读（paletteBaseline 负责开→读→关）；
       旧导出 overlayText() 在浮层未打开时恒返回空串，排除守卫永不生效，故移除。 */
    paletteBaseline: paletteBaseline, paletteGroups: paletteGroups, articleGroupItems: articleGroupItems,
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
