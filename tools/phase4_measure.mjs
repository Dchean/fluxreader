// TASK-117（四阶段）：实机性能测量——对真实运行的应用做自动化测量电池。
// 前置：应用以远程调试端口启动：
//   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 FluxReader.exe
// 运行：node tools/phase4_measure.mjs --out tmp/phase4/measure-report.json
// 依赖：tools/t059_cdp.mjs 的连接基建（findPageTarget/connect）。
// 纪律：本脚本只读测量（滚动/点击/读堆内存），不写应用数据；DB 注入由 phase4_seed.py 负责。
import { writeFileSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { connect, findPageTarget, CDP_PORT } from './t059_cdp.mjs';

const outArg = process.argv.indexOf('--out');
const OUT = outArg > -1 ? resolve(process.argv[outArg + 1]) : 'tmp/phase4/measure-report.json';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function main() {
  const target = await findPageTarget(CDP_PORT);
  if (!target) throw new Error('未找到应用页面：确认应用已带 --remote-debugging-port=9222 启动');
  const cdp = await connect(target.webSocketDebuggerUrl);
  const send = (method, params = {}) => cdp.send(method, params);
  const evalJs = async (expr) => await cdp.evaluate(expr);

  const report = { capturedAt: new Date().toISOString(), port: CDP_PORT, measures: {} };

  // ---- M1 内存基线（各布局切换后采样）----
  const layouts = ['文章', '社交', '画廊', '播客', '通知'];
  report.measures.memory = [];
  for (const name of layouts) {
    await evalJs(`(() => { const b=[...document.querySelectorAll('button')].find(x=>x.textContent.trim()===${JSON.stringify(name)}); if(b) b.click(); })()`);
    await sleep(1200);
    const mem = await evalJs(`performance.memory ? { used: performance.memory.usedJSHeapSize, limit: performance.memory.jsHeapSizeLimit } : null`);
    const cards = await evalJs(`document.querySelectorAll('[data-card-index]').length`);
    report.measures.memory.push({ layout: name, heapUsedMB: mem ? +(mem.used / 1048576).toFixed(1) : null, cards });
  }

  // ---- M2 滚动流畅度（文章布局：3 秒程序化滚动，采样 rAF 帧间隔）----
  await evalJs(`(() => { const b=[...document.querySelectorAll('button')].find(x=>x.textContent.trim()==='文章'); if(b) b.click(); })()`);
  await sleep(1000);
  const frameStats = await evalJs(`(async () => {
    let cards = document.querySelectorAll('[data-card-index]').length;
    if (!cards) { for (const n of ['社交','通知','播客','画廊']) { const b=[...document.querySelectorAll('button')].find(x=>x.textContent.trim()===n); if(b) b.click(); await new Promise(r=>setTimeout(r,800)); cards = document.querySelectorAll('[data-card-index]').length; if (cards) break; } }
    const scrollers = [...document.querySelectorAll('div')].filter(d => d.scrollHeight > d.clientHeight + 200 && d.clientHeight > 300);
    const list = scrollers.sort((a, b) => b.scrollHeight - a.scrollHeight)[0];
    if (!list) return { error: 'no-scrollable-list' };
    const deltas = []; let last = performance.now(); let raf;
    const tick = (t) => { deltas.push(t - last); last = t; if (t - start < 3000) raf = requestAnimationFrame(tick); };
    const start = performance.now(); raf = requestAnimationFrame(tick);
    const scrollTimer = setInterval(() => { list.scrollTop = Math.min(list.scrollTop + 240, list.scrollHeight); }, 100);
    await new Promise((r) => setTimeout(r, 3100));
    clearInterval(scrollTimer); cancelAnimationFrame(raf);
    const sorted = [...deltas].sort((a, b) => a - b);
    const p = (q) => +(sorted[Math.floor(sorted.length * q)] || 0).toFixed(1);
    return { frames: deltas.length, p50: p(0.5), p95: p(0.95), max: +Math.max(...deltas).toFixed(1), jankOver50ms: deltas.filter(d => d > 50).length, listHeight: list.scrollHeight };
  })()`);
  report.measures.scrollArticle = frameStats;

  // ---- M3 切换延迟（点击订阅源/视图 → 列表重渲染完成）----
  report.measures.switchLatency = [];
  const feedButtons = await evalJs(`[...document.querySelectorAll('button')].filter(b => /\\d$/.test(b.textContent.trim()) && b.closest('[class*=feed], li, [class*=sidebar]')).slice(0, 3).map(b => b.textContent.trim())`);
  const hasCards = await evalJs(`document.querySelectorAll('[data-card-index]').length > 0`);
  for (const label of (hasCards ? (feedButtons || []).slice(0, 3) : [])) {
    const t0 = Date.now();
    await evalJs(`(() => { const b=[...document.querySelectorAll('button')].find(x=>x.textContent.trim()===${JSON.stringify(label)}); if(b) b.click(); })()`);
    await evalJs(`new Promise((res) => { const t0=Date.now(); const iv=setInterval(()=>{ const done=document.querySelectorAll('[data-card-index]').length>0 || Date.now()-t0>8000; if(done){clearInterval(iv);res();} },50); })`);
    report.measures.switchLatency.push({ target: label, ms: Date.now() - t0 });
    await sleep(400);
  }
  if (!hasCards) report.measures.switchLatency.push({ skipped: '当前布局无数据卡片（feed 绑定其他布局），延迟测量在绑定布局轮执行' });

  // ---- M4 搜索延迟（Ctrl+K → 输入 → 结果出现）----
  await evalJs(`window.dispatchEvent(new KeyboardEvent('keydown', { key: 'k', code: 'KeyK', ctrlKey: true, bubbles: true }))`);
  await sleep(500);
  const search = await evalJs(`(async () => {
    const input = document.querySelector('input[type=text], input:not([type])');
    if (!input) return { error: 'no-search-input' };
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    const t0 = performance.now();
    setter.call(input, '#000001'); input.dispatchEvent(new Event('input', { bubbles: true }));
    const t0b = performance.now();
    await new Promise((res) => {
      const iv = setInterval(() => {
        const overlay = document.body.textContent;
        const done = /000001/.test(overlay) && (performance.now() - t0b > 150);
        if (done || performance.now() - t0 > 5000) { clearInterval(iv); res(); }
      }, 50);
    });
    return { typedMs: Math.round(t0b - t0), resultMs: Math.round(performance.now() - t0) };
  })()`);
  report.measures.search = search;
  await evalJs(`window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))`);

  // ---- M5 DB 规模口径（供解读：文章量以 UI 计数为准，精确值看库）----
  report.measures.scaleNote = '精确文章数以 phase4_seed.py --info 或 sqlite COUNT 为准；本脚本记录 UI 可见卡片数与内存/帧率供对照。';

  mkdirSync(dirname(OUT), { recursive: true });
  writeFileSync(OUT, JSON.stringify(report, null, 1));
  console.log('report written:', OUT);
  console.log(JSON.stringify(report.measures, null, 1));
  process.exit(0);
}

main().catch((e) => { console.error('MEASURE FAILED:', e.message); process.exit(1); });
