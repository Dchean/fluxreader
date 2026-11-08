// 真实浏览器布局回归（Windows Edge/Chrome + CDP，无额外 devDependency）。
//
// 证据边界：
// - 启动仓库自身的 Vite dev server 与 headless Chromium，不访问真实 IPC/DB；
// - 页面数据经 store 注入，只验证布局几何与水合时序，不验证 Tauri/WebView2 差异；
// - 断言失败时仍尽量保存 metrics/截图，便于复核具体帧。

import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { connectPage, evalValue, navigate, saveJson, screenshot, setViewport, waitFor } from './ui-layout/cdp.mjs';
import { injectTimelineTrace } from './ui-layout/g2-timeline-inject.mjs';

const REPO_ROOT = fileURLToPath(new URL('../', import.meta.url));
const EVIDENCE_REL = process.env.UI_LAYOUT_EVIDENCE_DIR ?? 'tmp/ui-layout-evidence';
const EVIDENCE_DIR = path.resolve(REPO_ROOT, EVIDENCE_REL);
const TMP_ROOT = path.resolve(REPO_ROOT, 'tmp');
if (!EVIDENCE_DIR.startsWith(`${TMP_ROOT}${path.sep}`)) {
  throw new Error(`证据目录必须位于仓库 tmp/ 下：${EVIDENCE_DIR}`);
}
const MUTATION = process.env.UI_LAYOUT_MUTATION ?? 'none';
const CHECK_GROUP = process.env.UI_LAYOUT_CHECK_GROUP ?? 'all';
if (!['all', 'gallery'].includes(CHECK_GROUP)) throw new Error(`未知 UI_LAYOUT_CHECK_GROUP：${CHECK_GROUP}`);
/* G2 诊断插桩（临时诊断提交，采集后随修复一并撤除）：默认开启，UI_LAYOUT_TRACE=0 可关闭。
   仅 tools 层生效——页面运行时经 Page.addScriptToEvaluateOnNewDocument 注入；
   Timeline 事件点在 vite dev 服务的模块响应文本上注入，产品源码零改动。 */
const G2_TRACE = (process.env.UI_LAYOUT_TRACE ?? '1') === '1';
const G2_TRACE_RUNTIME_PATH = path.join(REPO_ROOT, 'tools', 'ui-layout', 'g2-trace-runtime.js');
const G2_TRACE_INJECTOR_PATH = path.join(REPO_ROOT, 'tools', 'ui-layout', 'g2-timeline-inject.mjs');
if (G2_TRACE && !fs.existsSync(G2_TRACE_RUNTIME_PATH)) throw new Error(`G2 插桩缺少运行时：${G2_TRACE_RUNTIME_PATH}`);
if (G2_TRACE && !fs.existsSync(G2_TRACE_INJECTOR_PATH)) throw new Error(`G2 插桩缺少模块注入器：${G2_TRACE_INJECTOR_PATH}`);
const G2_TRACE_SOURCE = G2_TRACE ? fs.readFileSync(G2_TRACE_RUNTIME_PATH, 'utf8') : null;
const HYDRATE_DELAY_MS = 1300;
const SAMPLE_MS = 2800;
const VIEWPORTS = [
  { label: '1440x900', width: 1440, height: 900 },
  { label: '980x640', width: 980, height: 640 },
];
const KEYBOARD_VIEWPORTS = [
  { label: '1536x900', width: 1536, height: 900 },
  { label: '1280x800', width: 1280, height: 800 },
  { label: '980x640', width: 980, height: 640 },
];
const STARTUP_TIMEOUT_MS = 120000;
const FAULT_MODE = process.env.UI_LAYOUT_FAULT ?? 'none';
const FAULT_DELAY_RAW = Number(process.env.UI_LAYOUT_FAULT_DELAY_MS ?? 25000);
if (!['none', 'bootstrap-error'].includes(FAULT_MODE)) {
  throw new Error(`UI_LAYOUT_FAULT 仅支持 none / bootstrap-error：${FAULT_MODE}`);
}
if (!Number.isFinite(FAULT_DELAY_RAW) || FAULT_DELAY_RAW < 1000) {
  throw new Error(`UI_LAYOUT_FAULT_DELAY_MS 必须是不小于 1000 的毫秒数：${process.env.UI_LAYOUT_FAULT_DELAY_MS}`);
}
const FAULT_DELAY_MS = FAULT_DELAY_RAW;
class StartupTimeoutError extends Error {
  constructor(message, details = {}) {
    super(message);
    this.name = 'StartupTimeoutError';
    Object.assign(this, details);
  }
}
const SELECTED_VIEWPORTS = process.env.UI_LAYOUT_VIEWPORTS
  ? VIEWPORTS.filter((viewport) => process.env.UI_LAYOUT_VIEWPORTS.split(',').includes(viewport.label))
  : VIEWPORTS;
if (SELECTED_VIEWPORTS.length === 0) throw new Error('UI_LAYOUT_VIEWPORTS 未匹配任何视口');
const SELECTED_KEYBOARD_VIEWPORTS = process.env.UI_LAYOUT_VIEWPORTS
  ? KEYBOARD_VIEWPORTS.filter((viewport) => process.env.UI_LAYOUT_VIEWPORTS.split(',').includes(viewport.label))
  : KEYBOARD_VIEWPORTS;
const SOCIAL_ID_BASE = { '1440x900': 9001, '980x640': 10001 };

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function freePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.on('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      const port = typeof address === 'object' && address ? address.port : null;
      server.close((error) => (error ? reject(error) : resolve(port)));
    });
  });
}

async function waitHttp(url, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
      lastError = new Error(`HTTP ${response.status}`);
    } catch (error) {
      lastError = error;
    }
    await delay(100);
  }
  throw new Error(`等待 ${url} 超时：${lastError?.message ?? 'unknown'}`);
}

async function waitDevTools(debugPort, timeoutMs = 20000) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${debugPort}/json/version`);
      if (response.ok) return;
      lastError = new Error(`HTTP ${response.status}`);
    } catch (error) {
      lastError = error;
    }
    await delay(100);
  }
  throw new Error(`等待浏览器调试端口超时：${lastError?.message ?? 'unknown'}`);
}

const MEDIA_SVG = (width, height, label) => Buffer.from(
  '<svg xmlns="http://www.w3.org/2000/svg" width="' + width + '" height="' + height + '">'
  + '<rect width="100%" height="100%" fill="#4d6f93"/>'
  + '<text x="24" y="48" fill="white" font-size="28">' + label + '</text></svg>',
);
/* 3.7KB no-op MP4：只用于让 Chromium 真实发出视频元数据事件；来源为本机
   浏览器扩展的 noop 占位资源，不依赖运行机器路径。 */
const MEDIA_VIDEO_MP4 = Buffer.from(
  'AAAAHGZ0eXBNNFYgAAACAGlzb21pc28yYXZjMQAAAAhmcmVlAAAGF21kYXTeBAAAbGliZmFhYyAxLjI4AABCAJMgBDIARwAAArEGBf//rdxF6b3m2Ui3lizYINkj7u94MjY0IC0gY29yZSAxNDIgcjIgOTU2YzhkOCAtIEguMjY0L01QRUctNCBBVkMgY29kZWMgLSBDb3B5bGVmdCAyMDAzLTIwMTQgLSBodHRwOi8vd3d3LnZpZGVvbGFuLm9yZy94MjY0Lmh0bWwgLSBvcHRpb25zOiBjYWJhYz0wIHJlZj0zIGRlYmxvY2s9MTowOjAgYW5hbHlzZT0weDE6MHgxMTEgbWU9aGV4IHN1Ym1lPTcgcHN5PTEgcHN5X3JkPTEuMDA6MC4wMCBtaXhlZF9yZWY9MSBtZV9yYW5nZT0xNiBjaHJvbWFfbWU9MSB0cmVsbGlzPTEgOHg4ZGN0PTAgY3FtPTAgZGVhZHpvbmU9MjEsMTEgZmFzdF9wc2tpcD0xIGNocm9tYV9xcF9vZmZzZXQ9LTIgdGhyZWFkcz02IGxvb2thaGVhZF90aHJlYWRzPTEgc2xpY2VkX3RocmVhZHM9MCBucj0wIGRlY2ltYXRlPTEgaW50ZXJsYWNlZD0wIGJsdXJheV9jb21wYXQ9MCBjb25zdHJhaW5lZF9pbnRyYT0wIGJmcmFtZXM9MCB3ZWlnaHRwPTAga2V5aW50PTI1MCBrZXlpbnRfbWluPTI1IHNjZW5lY3V0PTQwIGludHJhX3JlZnJlc2g9MCByY19sb29rYWhlYWQ9NDAgcmM9Y3JmIG1idHJlZT0xIGNyZj0yMy4wIHFjb21wPTAuNjAgcXBtaW49MCBxcG1heD02OSBxcHN0ZXA9NCB2YnZfbWF4cmF0ZT03NjggdmJ2X2J1ZnNpemU9MzAwMCBjcmZfbWF4PTAuMCBuYWxfaHJkPW5vbmUgZmlsbGVyPTAgaXBfcmF0aW89MS40MCBhcT0xOjEuMDAAgAAAAFZliIQL8mKAAKvMnJycnJycnJycnXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXiEASZACGQAjgCEASZACGQAjgAAAAAdBmjgX4GSAIQBJkAIZACOAAAAAB0GaVAX4GSAhAEmQAhkAI4AhAEmQAhkAI4AAAAAGQZpgL8DJIQBJkAIZACOAIQBJkAIZACOAAAAABkGagC/AySEASZACGQAjgAAAAAZBmqAvwMkhAEmQAhkAI4AhAEmQAhkAI4AAAAAGQZrAL8DJIQBJkAIZACOAAAAABkGa4C/AySEASZACGQAjgCEASZACGQAjgAAAAAZBmwAvwMkhAEmQAhkAI4AAAAAGQZsgL8DJIQBJkAIZACOAIQBJkAIZACOAAAAABkGbQC/AySEASZACGQAjgCEASZACGQAjgAAAAAZBm2AvwMkhAEmQAhkAI4AAAAAGQZuAL8DJIQBJkAIZACOAIQBJkAIZACOAAAAABkGboC/AySEASZACGQAjgAAAAAZBm8AvwMkhAEmQAhkAI4AhAEmQAhkAI4AAAAAGQZvgL8DJIQBJkAIZACOAAAAABkGaAC/AySEASZACGQAjgCEASZACGQAjgAAAAAZBmiAvwMkhAEmQAhkAI4AhAEmQAhkAI4AAAAAGQZpAL8DJIQBJkAIZACOAAAAABkGaYC/AySEASZACGQAjgCEASZACGQAjgAAAAAZBmoAvwMkhAEmQAhkAI4AAAAAGQZqgL8DJIQBJkAIZACOAIQBJkAIZACOAAAAABkGawC/AySEASZACGQAjgAAAAAZBmuAvwMkhAEmQAhkAI4AhAEmQAhkAI4AAAAAGQZsAL8DJIQBJkAIZACOAAAAABkGbIC/AySEASZACGQAjgCEASZACGQAjgAAAAAZBm0AvwMkhAEmQAhkAI4AhAEmQAhkAI4AAAAAGQZtgL8DJIQBJkAIZACOAAAAABkGbgCvAySEASZACGQAjgCEASZACGQAjgAAAAAZBm6AnwMkhAEmQAhkAI4AhAEmQAhkAI4AhAEmQAhkAI4AhAEmQAhkAI4AAAAhubW9vdgAAAGxtdmhkAAAAAAAAAAAAAAAAAAAD6AAABDcAAQAAAQAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAwAAAzB0cmFrAAAAXHRraGQAAAADAAAAAAAAAAAAAAABAAAAAAAAA+kAAAAAAAAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAABAAAAAALAAAACQAAAAAAAkZWR0cwAAABxlbHN0AAAAAAAAAAEAAAPpAAAAAAABAAAAAAKobWRpYQAAACBtZGhkAAAAAAAAAAAAAAAAAAB1MAAAdU5VxAAAAAAALWhkbHIAAAAAAAAAAHZpZGUAAAAAAAAAAAAAAABWaWRlb0hhbmRsZXIAAAACU21pbmYAAAAUdm1oZAAAAAEAAAAAAAAAAAAAACRkaW5mAAAAHGRyZWYAAAAAAAAAAQAAAAx1cmwgAAAAAQAAAhNzdGJsAAAAr3N0c2QAAAAAAAAAAQAAAJ9hdmMxAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAAAALAAkABIAAAASAAAAAAAAAABAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAGP//AAAALWF2Y0MBQsAN/+EAFWdCwA3ZAsTsBEAAAPpAADqYA8UKkgEABWjLg8sgAAAAHHV1aWRraEDyXyRPxbo5pRvPAyPzAAAAAAAAABhzdHRzAAAAAAAAAAEAAAAeAAAD6QAAABRzdHNzAAAAAAAAAAEAAAABAAAAHHN0c2MAAAAAAAAAAQAAAAEAAAABAAAAAQAAAIxzdHN6AAAAAAAAAAAAAAAeAAADDwAAAAsAAAALAAAACgAAAAoAAAAKAAAACgAAAAoAAAAKAAAACgAAAAoAAAAKAAAACgAAAAoAAAAKAAAACgAAAAoAAAAKAAAACgAAAAoAAAAKAAAACgAAAAoAAAAKAAAACgAAAAoAAAAKAAAACgAAAAoAAAAKAAAAiHN0Y28AAAAAAAAAHgAAAEYAAANnAAADewAAA5gAAAO0AAADxwAAA+MAAAP2AAAEEgAABCUAAARBAAAEXQAABHAAAASMAAAEnwAABLsAAATOAAAE6gAABQYAAAUZAAAFNQAABUgAAAVkAAAFdwAABZMAAAWmAAAFwgAABd4AAAXxAAAGDQAABGh0cmFrAAAAXHRraGQAAAADAAAAAAAAAAAAAAACAAAAAAAABDcAAAAAAAAAAAAAAAEBAAAAAAEAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAAAAkZWR0cwAAABxlbHN0AAAAAAAAAAEAAAQkAAADcAABAAAAAAPgbWRpYQAAACBtZGhkAAAAAAAAAAAAAAAAAAC7gAAAykBVxAAAAAAALWhkbHIAAAAAAAAAAHNvdW4AAAAAAAAAAAAAAABTb3VuZEhhbmRsZXIAAAADi21pbmYAAAAQc21oZAAAAAAAAAAAAAAAJGRpbmYAAAAcZHJlZgAAAAAAAAABAAAADHVybCAAAAABAAADT3N0YmwAAABnc3RzZAAAAAAAAAABAAAAV21wNGEAAAAAAAAAAQAAAAAAAAAAAAIAEAAAAAC7gAAAAAAAM2VzZHMAAAAAA4CAgCIAAgAEgICAFEAVBbjYAAu4AAAADcoFgICAAhGQBoCAgAECAAAAIHN0dHMAAAAAAAAAAgAAADIAAAQAAAAAAQAAAkAAAAFUc3RzYwAAAAAAAAAbAAAAAQAAAAEAAAABAAAAAgAAAAIAAAABAAAAAwAAAAEAAAABAAAABAAAAAIAAAABAAAABgAAAAEAAAABAAAABwAAAAIAAAABAAAACAAAAAEAAAABAAAACQAAAAIAAAABAAAACgAAAAEAAAABAAAACwAAAAIAAAABAAAADQAAAAEAAAABAAAADgAAAAIAAAABAAAADwAAAAEAAAABAAAAEAAAAAIAAAABAAAAEQAAAAEAAAABAAAAEgAAAAIAAAABAAAAFAAAAAEAAAABAAAAFQAAAAIAAAABAAAAFgAAAAEAAAABAAAAFwAAAAIAAAABAAAAGAAAAAEAAAABAAAAGQAAAAIAAAABAAAAGgAAAAEAAAABAAAAGwAAAAIAAAABAAAAHQAAAAEAAAABAAAAHgAAAAIAAAABAAAAHwAAAAQAAAABAAAA4HN0c3oAAAAAAAAAAAAAADMAAAAaAAAACQAAAAkAAAAJAAAACQAAAAkAAAAJAAAACQAAAAkAAAAJAAAACQAAAAkAAAAJAAAACQAAAAkAAAAJAAAACQAAAAkAAAAJAAAACQAAAAkAAAAJAAAACQAAAAkAAAAJAAAACQAAAAkAAAAJAAAACQAAAAkAAAAJAAAACQAAAAkAAAAJAAAACQAAAAkAAAAJAAAACQAAAAkAAAAJAAAACQAAAAkAAAAJAAAACQAAAAkAAAAJAAAACQAAAAkAAAAJAAAACQAAACMc3RjbwAAAAAAAAAfAAAALAAAA1UAAANyAAADhgAAA6IAAAO+AAAD0QAAA+0AAAQAAAAEHAAABC8AAARLAAAEZwAABHoAAASWAAAEqQAABMUAAATYAAAE9AAABRAAAAUjAAAFPwAABVIAAAVuAAAFgQAABZ0AAAWwAAAFzAAABegAAAX7AAAGFwAAAGJ1ZHRhAAAAWm1ldGEAAAAAAAAAIWhkbHIAAAAAAAAAAG1kaXJhcHBsAAAAAAAAAAAAAAAALWlsc3QAAAAlqXRvbwAAAB1kYXRhAAAAAQAAAABMYXZmNTUuMzMuMTAw'
  , 'base64',
);

async function startMediaServer() {
  const port = await freePort();
  const requests = [];
  const galleryReleased = new Set();
  const galleryPending = new Map();
  const releaseGalleryWave = (wave) => {
    galleryReleased.add(wave);
    const pending = galleryPending.get(wave) ?? [];
    galleryPending.delete(wave);
    pending.forEach((respond, index) => setTimeout(respond, 60 + (index % 4) * 55));
  };
  const server = http.createServer((request, response) => {
    const url = new URL(request.url ?? '/', 'http://127.0.0.1');
    const delayMs = Math.max(0, Math.min(3000, Number(url.searchParams.get('delay') ?? 600)));
    const startedAt = Date.now();
    const finish = (status, headers, body) => {
      const bytes = Buffer.isBuffer(body) ? body : Buffer.from(String(body));
      response.writeHead(status, {
        'Access-Control-Allow-Origin': '*',
        'Cache-Control': 'no-store',
        ...headers,
        'Content-Length': bytes.length,
      });
      response.end(bytes);
      requests.push({
        path: request.url ?? '/',
        status,
        delayMs: url.pathname === '/gallery-image.svg' ? null : delayMs,
        galleryWave: url.pathname === '/gallery-image.svg' ? url.searchParams.get('wave') : null,
        startedAt,
        finishedAt: Date.now(),
        bytes: bytes.length,
        range: request.headers.range ?? null,
      });
    };
    if (url.pathname === '/gallery-image.svg') {
      const wave = url.searchParams.get('wave');
      const width = Number(url.searchParams.get('width'));
      const height = Number(url.searchParams.get('height'));
      const respond = () => finish(200, { 'Content-Type': 'image/svg+xml' }, MEDIA_SVG(width, height, 'GALLERY_HTTP_MARKER'));
      if (galleryReleased.has(wave)) setTimeout(respond, 60);
      else galleryPending.set(wave, [...(galleryPending.get(wave) ?? []), respond]);
      return;
    }
    setTimeout(() => {
      if (url.pathname === '/delayed-image.svg') {
        finish(200, { 'Content-Type': 'image/svg+xml' }, MEDIA_SVG(800, 1100, 'HTTP_IMAGE_MARKER'));
        return;
      }
      if (url.pathname === '/failed-image.svg') {
        finish(404, { 'Content-Type': 'text/plain' }, 'missing image');
        return;
      }
      if (url.pathname === '/poster.svg') {
        finish(200, { 'Content-Type': 'image/svg+xml' }, MEDIA_SVG(800, 1100, 'POSTER_MARKER'));
        return;
      }
      if (url.pathname === '/video.mp4') {
        const total = MEDIA_VIDEO_MP4.length;
        const range = request.headers.range;
        if (range) {
          const match = /^bytes=(\d*)-(\d*)$/.exec(range);
          const start = match && match[1] ? Number(match[1]) : 0;
          const end = match && match[2] ? Number(match[2]) : total - 1;
          if (start >= total || end < start) {
            finish(416, { 'Content-Range': `bytes */${total}`, 'Content-Type': 'video/mp4' }, Buffer.alloc(0));
            return;
          }
          const slice = MEDIA_VIDEO_MP4.subarray(start, Math.min(end + 1, total));
          finish(206, {
            'Content-Type': 'video/mp4',
            'Accept-Ranges': 'bytes',
            'Content-Range': `bytes ${start}-${start + slice.length - 1}/${total}`,
          }, slice);
          return;
        }
        finish(200, { 'Content-Type': 'video/mp4', 'Accept-Ranges': 'bytes' }, MEDIA_VIDEO_MP4);
        return;
      }
      finish(404, { 'Content-Type': 'text/plain' }, 'missing media');
    }, delayMs);
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, '127.0.0.1', resolve);
  });
  return {
    url: `http://127.0.0.1:${port}`,
    requests,
    releaseGalleryWave,
    close: () => {
      for (const wave of galleryPending.keys()) releaseGalleryWave(wave);
      return new Promise((resolve) => server.close(resolve));
    },
  };
}

function findBrowser() {
  const candidates = [
    process.env.EDGE_PATH,
    process.env.CHROME_PATH,
    'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
    'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
    'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
    'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
  ].filter(Boolean);
  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) return candidate;
  }
  throw new Error('未找到 Edge/Chrome；可用 EDGE_PATH 或 CHROME_PATH 指定可执行文件');
}

function terminateTree(child) {
  if (!child || child.exitCode !== null || !child.pid) return;
  if (process.platform === 'win32') {
    spawnSync('taskkill', ['/pid', String(child.pid), '/T', '/F'], { stdio: 'ignore', windowsHide: true });
  } else {
    child.kill('SIGTERM');
  }
}

async function startVite() {
  const port = await freePort();
  const viteEntry = path.join(REPO_ROOT, 'node_modules', 'vite', 'bin', 'vite.js');
  if (!fs.existsSync(viteEntry)) throw new Error(`缺少 Vite 入口：${viteEntry}`);
  const child = spawn(process.execPath, [
    viteEntry,
    '--host', '127.0.0.1',
    '--port', String(port),
    '--strictPort',
    '--logLevel', 'error',
  ], {
    cwd: REPO_ROOT,
    env: { ...process.env, BROWSER: 'none', NO_COLOR: '1' },
    stdio: ['ignore', 'pipe', 'pipe'],
    windowsHide: true,
  });
  let output = '';
  child.stdout.on('data', (chunk) => { output += chunk.toString(); });
  child.stderr.on('data', (chunk) => { output += chunk.toString(); });
  try {
    await waitHttp(`http://127.0.0.1:${port}/`, STARTUP_TIMEOUT_MS);
  } catch (error) {
    terminateTree(child);
    throw new StartupTimeoutError(`${error.message}\nVite 输出：\n${output}`, {
      phase: 'Vite 启动',
      cause: error,
    });
  }
  return { child, port, url: `http://127.0.0.1:${port}/`, getOutput: () => output };
}

async function startBrowser(profileDir) {
  const debugPort = await freePort();
  const browserPath = findBrowser();
  const child = spawn(browserPath, [
    '--headless=new',
    `--remote-debugging-port=${debugPort}`,
    `--user-data-dir=${profileDir}`,
    '--no-first-run',
    '--no-default-browser-check',
    '--disable-gpu',
    '--disable-extensions',
    '--disable-background-networking',
    '--disable-component-update',
    '--disable-sync',
    '--mute-audio',
    'about:blank',
  ], {
    cwd: REPO_ROOT,
    stdio: 'ignore',
    windowsHide: true,
  });
  try {
    await waitDevTools(debugPort, STARTUP_TIMEOUT_MS);
  } catch (error) {
    terminateTree(child);
    throw new StartupTimeoutError(error.message, {
      phase: '浏览器调试端口',
      cause: error,
    });
  }
  return { child, debugPort, browserPath };
}

async function readBootstrapError(cdp) {
  try {
    return await evalValue(cdp, `(() => {
      const node = document.querySelector('.bootstrap-error');
      if (!node) return null;
      const text = node.querySelector('.bootstrap-error-text')?.textContent
        ?? node.textContent
        ?? '';
      return text.trim().slice(0, 500) || '存在 .bootstrap-error，但无文本';
    })()`);
  } catch {
    return null;
  }
}

async function readStartupPageState(cdp) {
  try {
    return await evalValue(cdp, `({
      readyState: document.readyState,
      url: location.href,
      root: !!document.querySelector('.app-root'),
      splash: !!document.querySelector('.app-loading-splash'),
      bootstrapError: !!document.querySelector('.bootstrap-error'),
      bodyText: document.body.innerText.slice(0, 300),
      fault: window.__uiLayoutFault ? {
        scheduledAt: window.__uiLayoutFault.scheduledAt,
        injectedAt: window.__uiLayoutFault.injectedAt,
        elapsedMs: window.__uiLayoutFault.elapsedMs,
        delayMs: window.__uiLayoutFault.delayMs,
      } : null,
    })`);
  } catch {
    return null;
  }
}

async function waitForStartupPhase(cdp, expression, phase, timeoutMs = STARTUP_TIMEOUT_MS) {
  const startedAt = performance.now();
  const budgetMs = Math.max(1, timeoutMs);
  const deadline = Date.now() + budgetMs;
  let last = null;
  while (Date.now() < deadline) {
    /* .bootstrap-error 出现后，splash 可能仍在，普通等待条件永远不完成；
       因此每个采样周期先读错误，发现即中止本次启动，不再等满预算。 */
    const bootstrapError = await readBootstrapError(cdp);
    if (bootstrapError) {
      const endedAt = performance.now();
      const pageState = await readStartupPageState(cdp);
      const elapsedMs = endedAt - startedAt;
      throw new StartupTimeoutError(
        `${phase} 期间检测到 .bootstrap-error（${Math.round(elapsedMs)}ms）：${bootstrapError}`,
        {
          phase,
          cause: new Error('bootstrap error detected'),
          budgetMs,
          startedAt,
          endedAt,
          elapsedMs,
          bootstrapError,
          pageState,
          detectedEarly: true,
        },
      );
    }
    try {
      last = await evalValue(cdp, `Boolean(${expression})`);
      if (last) {
        const endedAt = performance.now();
        return { startedAt, endedAt, elapsedMs: endedAt - startedAt };
      }
    } catch (error) {
      last = error.message;
    }
    await delay(50);
  }
  const bootstrapError = await readBootstrapError(cdp);
  const pageState = await readStartupPageState(cdp);
  const endedAt = performance.now();
  const elapsedMs = endedAt - startedAt;
  const bootstrapText = bootstrapError
    ? `检测到 .bootstrap-error：${bootstrapError}`
    : '未检测到 .bootstrap-error';
  throw new StartupTimeoutError(
    `${phase} 超时（${Math.round(elapsedMs)}ms，预算 ${Math.round(budgetMs)}ms）；${bootstrapText}`,
    {
      phase,
      cause: new Error(`timeout waiting for ${expression}; last=${last}`),
      budgetMs,
      startedAt,
      endedAt,
      elapsedMs,
      bootstrapError,
      pageState,
      detectedEarly: Boolean(bootstrapError),
    },
  );
}

async function installStartupFault(cdp, delayMs) {
  await evalValue(cdp, `(() => {
    const scheduledAt = performance.now();
    window.__uiLayoutFault = {
      scheduledAt,
      delayMs: ${delayMs},
      injectedAt: null,
      elapsedMs: null,
    };
    setTimeout(() => {
      const error = document.createElement('div');
      error.className = 'bootstrap-error';
      const text = document.createElement('div');
      text.className = 'bootstrap-error-text';
      text.textContent = '故障注入：启动失败（ui-layout）';
      error.appendChild(text);
      document.body.appendChild(error);
      if (!document.querySelector('.app-loading-splash')) {
        const splash = document.createElement('div');
        splash.className = 'app-loading-splash';
        splash.textContent = '故障注入：保留 splash';
        document.body.appendChild(splash);
      }
      window.__uiLayoutFault.injectedAt = performance.now();
      window.__uiLayoutFault.elapsedMs = performance.now() - scheduledAt;
    }, ${delayMs});
  })()`);
}

const LAYOUTS = ['article', 'social', 'image', 'podcast', 'notification'];

async function applyRuntimeMutation(cdp) {
  if (MUTATION === 'none') return;
  if (MUTATION === 'no-social-media-remeasure') {
    await evalValue(cdp, `(() => {
      const NativeResizeObserver = window.ResizeObserver;
      window.ResizeObserver = class extends NativeResizeObserver {
        observe(target, options) {
          if (target?.classList?.contains('social-text-inner')) return;
          return super.observe(target, options);
        }
      };
      const nativeAddEventListener = EventTarget.prototype.addEventListener;
      const mediaEventNames = ['load', 'error', 'loadedmetadata', 'loadeddata', 'durationchange'];
      EventTarget.prototype.addEventListener = function (type, listener, options) {
        const isSocialMediaTarget = this instanceof Element
          && (this.classList.contains('social-text-inner')
            || this.closest('.social-text-inner') !== null);
        if (isSocialMediaTarget
          && mediaEventNames.includes(type)
          && !listener?.__socialAuditRecorder) return;
        return nativeAddEventListener.call(this, type, listener, options);
      };
      return true;
    })()`);
    return;
  }
  const css = MUTATION === 'no-social-clamp'
    ? '.social-text.collapsed { max-height: none !important; -webkit-line-clamp: unset !important; overflow: visible !important; }'
    : MUTATION === 'no-flex-shrink'
      ? '.timeline-virtual-wrap, .gallery-masonry-grid { flex-shrink: 1 !important; }'
      : null;
  if (!css) throw new Error(`未知 UI_LAYOUT_MUTATION：${MUTATION}`);
  await evalValue(cdp, `(() => {
    const style = document.createElement('style');
    style.id = 'ui-layout-mutation';
    style.textContent = ${JSON.stringify(css)};
    document.head.appendChild(style);
    return true;
  })()`);
}

async function installLayoutFixture(cdp) {
  await evalValue(cdp, `(async () => {
    const { useAppStore, selectVisibleEntries } = await import('/src/store.ts');
    window.__uiLayoutStore = useAppStore;
    window.__uiLayoutVisible = () => selectVisibleEntries(useAppStore.getState());
    const now = Date.now();
    const defs = [
      { layout: 'article', feedId: 'f-openai' },
      { layout: 'social', feedId: 'f-sama' },
      { layout: 'image', feedId: 'f-nasa' },
      { layout: 'podcast', feedId: 'f-hardfork' },
      { layout: 'notification', feedId: 'f-github' },
    ];
    const svg = (label, width, height, color) => 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(
      '<svg xmlns="http://www.w3.org/2000/svg" width="' + width + '" height="' + height + '">' +
      '<rect width="100%" height="100%" fill="' + color + '"/>' +
      '<text x="20" y="40" fill="white" font-size="24">' + label + '</text></svg>'
    );
    const longBody = '<p>首段说明。</p>' + Array.from({ length: 12 }, (_, index) =>
      '<p>正文段落 ' + index + '：用于验证折叠、虚拟化测量与页脚位置。</p>'
    ).join('');
    const entries = [];
    for (const def of defs) {
      for (let index = 0; index < 80; index++) {
        const id = def.layout + '-' + String(index + 1).padStart(3, '0');
        const entry = {
          id,
          feedId: def.feedId,
          title: def.layout + ' 条目 ' + String(index + 1).padStart(2, '0') + '：用于验证虚拟列表和底栏位置',
          snippet: '这是摘要文本，用来让卡片保持稳定高度。'.repeat(index % 6 === 0 ? 8 : 2),
          publishedAt: now - index * 60_000,
          source: 'direct',
          isRead: index % 3 === 0,
          isStarred: index % 4 === 0,
          tags: [],
          author: def.layout === 'social' ? 'Sam Altman' : 'Audit Author',
          content: def.layout === 'social' || def.layout === 'notification' ? longBody : '',
          rawContent: '',
          aiSummary: def.layout === 'notification' && index % 2 === 0 ? '自动摘要内容。' : '',
          translatedContent: def.layout === 'notification' && index % 3 === 0 ? '<p>Translated notification body</p>' : '',
        };
        if (def.layout === 'image') {
          entry.imageUrl = svg(id, 600, index % 2 === 0 ? 800 : 450, index % 2 === 0 ? '#315b7d' : '#7d5731');
        }
        if (def.layout === 'article') entry.cover = svg(id, 160, 120, '#3d4f7d');
        if (def.layout === 'podcast') {
          entry.cover = svg(id, 120, 120, '#315b4f');
          entry.durationSec = 900 + index * 17;
        }
        entries.push(entry);
      }
    }
    useAppStore.setState({
      dataMode: 'mock',
      entries,
      activeContentLayout: 'article',
      activeViewFilter: 'all',
      activeFeedFilter: 'all',
      timelineFilter: 'all',
      timelineSort: 'newest',
      articlesExhausted: true,
      articlesLoading: false,
      activeArticleId: null,
    });
    window.__uiLayoutSelect = async (layout) => {
      // 先归零再切布局；等待组件 effect 归零会让旧深滚位置短暂挂载并误发新图请求。
      const root = document.getElementById('timelineContentScroll');
      if (root) root.scrollTop = 0;
      useAppStore.setState({
        activeContentLayout: layout,
        activeViewFilter: 'all',
        activeFeedFilter: 'all',
        activeArticleId: null,
      });
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      await new Promise((resolve) => setTimeout(resolve, 320));
    };
  })()`);
}

async function measureLayoutTop(cdp, layout) {
  return evalValue(cdp, `(() => {
    const scroller = document.querySelector('#timelineContentScroll');
    const wrap = document.querySelector('.timeline-virtual-wrap, .gallery-masonry-grid');
    const footer = document.querySelector('.timeline-load-more');
    const scrollerRect = scroller?.getBoundingClientRect();
    const wrapRect = wrap?.getBoundingClientRect();
    const footerRect = footer?.getBoundingClientRect();
    const inlineHeight = wrap ? Number.parseFloat(wrap.style.height) : Number.NaN;
    return {
      layout: ${JSON.stringify(layout)},
      scrollTop: scroller?.scrollTop ?? null,
      scrollHeight: scroller?.scrollHeight ?? null,
      clientHeight: scroller?.clientHeight ?? null,
      maxScrollTop: scroller ? scroller.scrollHeight - scroller.clientHeight : null,
      wrapClass: wrap?.className ?? null,
      wrapInlineHeight: Number.isFinite(inlineHeight) ? inlineHeight : null,
      wrapUsedHeight: wrapRect?.height ?? null,
      footerTop: footerRect?.top ?? null,
      footerBottom: footerRect?.bottom ?? null,
      scrollerBottom: scrollerRect?.bottom ?? null,
      footerClearOfScroller: footerRect && scrollerRect ? footerRect.top >= scrollerRect.bottom - 1 : null,
      footerInScrollerViewport: footerRect && scrollerRect
        ? footerRect.top < scrollerRect.bottom && footerRect.bottom > scrollerRect.top
        : null,
      mountedCards: document.querySelectorAll('.timeline-virtual-item, .gallery-virtual-item').length,
    };
  })()`);
}

async function scrollLayoutToBottom(cdp) {
  return evalValue(cdp, `(async () => {
    const scroller = document.querySelector('#timelineContentScroll');
    let stable = 0;
    let previous = -1;
    for (let attempt = 0; attempt < 16; attempt++) {
      scroller.scrollTop = scroller.scrollHeight;
      await new Promise((resolve) => requestAnimationFrame(resolve));
      await new Promise((resolve) => setTimeout(resolve, 80));
      stable = Math.abs(scroller.scrollTop - previous) < 1 ? stable + 1 : 0;
      previous = scroller.scrollTop;
      if (stable >= 2) break;
    }
    const footer = document.querySelector('.timeline-load-more');
    const scrollerRect = scroller.getBoundingClientRect();
    const footerRect = footer?.getBoundingClientRect();
    return {
      scrollTop: scroller.scrollTop,
      scrollHeight: scroller.scrollHeight,
      clientHeight: scroller.clientHeight,
      maxScrollTop: scroller.scrollHeight - scroller.clientHeight,
      footerTop: footerRect?.top ?? null,
      footerBottom: footerRect?.bottom ?? null,
      scrollerBottom: scrollerRect.bottom,
      footerWithinScrollerBottom: footerRect ? footerRect.bottom <= scrollerRect.bottom + 1 : null,
      footerInScrollerViewport: footerRect
        ? footerRect.top < scrollerRect.bottom && footerRect.bottom > scrollerRect.top
        : null,
      footerText: footer?.textContent ?? null,
      mountedCards: document.querySelectorAll('.timeline-virtual-item, .gallery-virtual-item').length,
    };
  })()`);
}

async function runLayoutChecks(cdp, evidence, viewportLabel) {
  await installLayoutFixture(cdp);
  evidence.layouts = {};
  for (const layout of LAYOUTS) {
    await evalValue(cdp, `window.__uiLayoutSelect(${JSON.stringify(layout)})`);
    const top = await measureLayoutTop(cdp, layout);
    evidence.layouts[layout] = { top };
    await screenshot(cdp, path.join(EVIDENCE_DIR, `${viewportLabel}-layout-${layout}-top.png`));

    assert.ok(top.wrapInlineHeight > 0, `[${layout}] 虚拟容器应有内联完整高度`);
    assert.ok(top.wrapUsedHeight >= top.wrapInlineHeight - 2,
      `[${layout}] 虚拟容器实际高度 ${top.wrapUsedHeight} 不应被压缩到内联高度 ${top.wrapInlineHeight}`);
    assert.ok(top.scrollHeight > top.clientHeight, `[${layout}] 长列表应可滚动`);
    assert.equal(top.footerInScrollerViewport, false,
      `[${layout}] 首屏不应出现页脚；footerTop=${top.footerTop}, scrollerBottom=${top.scrollerBottom}`);
    assert.equal(top.footerClearOfScroller, true,
      `[${layout}] 首屏页脚必须整体位于滚动视口下缘之外；footerTop=${top.footerTop}, scrollerBottom=${top.scrollerBottom}`);

    const bottom = await scrollLayoutToBottom(cdp);
    evidence.layouts[layout] = { top, bottom };
    await screenshot(cdp, path.join(EVIDENCE_DIR, `${viewportLabel}-layout-${layout}-bottom.png`));
    assert.equal(bottom.footerInScrollerViewport, true,
      `[${layout}] 滚动到底后页脚应进入视口；footer=${bottom.footerTop}..${bottom.footerBottom}`);
    assert.equal(bottom.footerWithinScrollerBottom, true,
      `[${layout}] 到底页脚底边不得越过滚动视口下缘；footerBottom=${bottom.footerBottom}, scrollerBottom=${bottom.scrollerBottom}`);
    assert.ok(bottom.footerText?.includes('没有更多了'), `[${layout}] 页脚文案应为「没有更多了」`);
  }
}

async function runGalleryLoadingCheck(cdp, evidence, viewportLabel, media) {
  await installLayoutFixture(cdp);
  if (G2_TRACE) {
    const traceReady = await evalValue(cdp, 'Boolean(window.__g2Trace)');
    if (!traceReady) throw new Error('插桩运行时未注入（window.__g2Trace 缺失）');
    await evalValue(cdp, 'window.__g2Mark("fixture-installed")');
  }
  const runKey = `${viewportLabel}-${Date.now()}`;
  const waves = [runKey + '-1', runKey + '-2'];
  try {
    await runGalleryLoadingWaves(cdp, evidence, viewportLabel, media, runKey, waves);
  } finally {
    if (G2_TRACE) {
      try {
        await dumpG2Trace(cdp, viewportLabel, runKey, media);
      } catch (dumpError) {
        /* dump 失败不得掩盖原检查失败；CI 日志可见，metrics 记录仅成功路径可见。 */
        console.error(`[g2-trace] dump 失败：${dumpError?.stack ?? dumpError}`);
        evidence.g2TraceDumpError = String(dumpError?.stack ?? dumpError);
      }
    }
  }
}

const g2TraceAccumulator = {};
async function dumpG2Trace(cdp, viewportLabel, runKey, media) {
  await evalValue(cdp, 'window.__g2Sampling = false');
  const trace = await evalValue(cdp, 'window.__g2Trace.dump()');
  trace.config = { viewport: viewportLabel, runKey, waveOrder: 'natural' };
  trace.mediaRequests = media.requests.filter((request) => request.galleryWave !== null);
  trace.runtimeSource = 'tools/ui-layout/g2-trace-runtime.js（经 Page.addScriptToEvaluateOnNewDocument 注入）';
  trace.timelineModule = '/src/components/Timeline.tsx 由 tools/ui-layout/g2-timeline-inject.mjs 在 vite dev 响应文本上注入，产品源码未改动';
  await fsp.writeFile(path.join(EVIDENCE_DIR, `g2-trace-${viewportLabel}.json`), JSON.stringify(trace));
  g2TraceAccumulator[viewportLabel] = trace;
  await fsp.writeFile(path.join(EVIDENCE_DIR, 'g2-trace.json'), JSON.stringify(g2TraceAccumulator));
  await fsp.writeFile(path.join(EVIDENCE_DIR, 'g2-trace-runtime.used.js'), G2_TRACE_SOURCE);
  await evalValue(cdp, 'window.__g2Trace.clear()');
}

async function runGalleryLoadingWaves(cdp, evidence, viewportLabel, media, runKey, waves) {
  await evalValue(cdp, `(async () => {
    await window.__uiLayoutSelect('article');
    const store = window.__uiLayoutStore;
    const heights = [1100, 320, 800, 450, 1400, 540];
    const entries = store.getState().entries.map((entry, index) => entry.feedId === 'f-nasa' ? {
      ...entry,
      imageUrl: ${JSON.stringify(media.url)} + '/gallery-image.svg?width=600&height=' + heights[index % heights.length]
        + '&wave=' + ${JSON.stringify(waves)}[index % 2] + '&id=' + entry.id,
    } : entry);
    store.setState({ entries });
    await window.__uiLayoutSelect('image');
    const root = document.querySelector('#timelineContentScroll');
    root.scrollTop = 700;
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    window.__galleryReadAnchor = (id) => {
      const rootRect = root.getBoundingClientRect();
      const cards = [...root.querySelectorAll('.gallery-virtual-item')].map((element) => {
        const rect = element.getBoundingClientRect();
        const card = element.querySelector('.gallery-card');
        const image = element.querySelector('img');
        const imageRect = image?.getBoundingClientRect();
        return {
          id: card?.dataset.id,
          index: Number(element.dataset.index),
          lane: Number(element.dataset.galleryLane),
          top: rect.top - rootRect.top,
          bottom: rect.bottom - rootRect.top,
          height: rect.height,
          image: image ? {
            complete: image.complete,
            naturalWidth: image.naturalWidth,
            naturalHeight: image.naturalHeight,
            width: imageRect.width,
            height: imageRect.height,
            src: image.currentSrc,
          } : null,
        };
      });
      const anchor = id ? cards.find((card) => card.id === id)
        : cards.filter((card) => card.top >= 0 && card.top < root.clientHeight).sort((left, right) => left.top - right.top || left.index - right.index)[0];
      return {
        at: performance.now(),
        anchor: anchor ?? null,
        scrollTop: root.scrollTop,
        scrollHeight: root.scrollHeight,
        clientHeight: root.clientHeight,
        scrollWidth: root.scrollWidth,
        clientWidth: root.clientWidth,
        cards,
      };
    };
    window.__g2Mark?.('gallery-ready');
  })()`);
  const result = { waves, phases: [] };
  evidence.galleryLoading = result;
  for (const [index, wave] of waves.entries()) {
    if (index > 0) {
      await evalValue(cdp, `(async () => {
        document.querySelector('#timelineContentScroll').scrollTop += 160;
        await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      })()`);
    }
    const before = await evalValue(cdp, 'window.__galleryReadAnchor()');
    assert.ok(before.anchor, `[${viewportLabel}] 第 ${index + 1} 波加载前必须指定一张可见 anchor`);
    assert.ok(before.cards.some((card) => card.image && !card.image.complete), `[${viewportLabel}] 必须在真实 HTTP 图片仍 pending 时采样`);
    const phase = { before, frames: [] };
    result.phases.push(phase);
    if (G2_TRACE) await evalValue(cdp, `window.__g2Sampling = true; window.__g2Mark('wave-${index + 1}-before-release')`);
    media.releaseGalleryWave(wave);
    phase.frames = await evalValue(cdp, `(async () => {
      const frames = [];
      const until = performance.now() + 850;
      while (performance.now() < until) {
        await new Promise((resolve) => requestAnimationFrame(resolve));
        frames.push(window.__galleryReadAnchor(${JSON.stringify(before.anchor.id)}));
      }
      return frames;
    })()`);
    if (G2_TRACE) await evalValue(cdp, `window.__g2Sampling = false; window.__g2Mark('wave-${index + 1}-frames-end')`);
    phase.after = phase.frames.at(-1);
    phase.maxAnchorDrift = Math.max(...phase.frames.map((frame) => frame.anchor
      ? Math.abs(frame.anchor.top - before.anchor.top) : Number.POSITIVE_INFINITY));
    phase.settledAnchorDrift = phase.after.anchor
      ? phase.after.anchor.top - before.anchor.top : null;
    await screenshot(cdp, path.join(EVIDENCE_DIR, `${viewportLabel}-gallery-http-wave-${index + 1}.png`));
    assert.ok(phase.after.cards.some((card) => card.image?.naturalWidth > 0), `[${viewportLabel}] HTTP 图片必须真实解码出自然尺寸`);
    assert.ok(phase.maxAnchorDrift <= 3,
      `[${viewportLabel}] 延迟图片第 ${index + 1} 波使 anchor ${before.anchor.id} 漂移 ${phase.maxAnchorDrift}px（最终 ${phase.settledAnchorDrift}px）`);
    assert.ok(phase.frames.every((frame) => frame.scrollWidth <= frame.clientWidth + 1), `[${viewportLabel}] 异步测量期间不可横向溢出`);
  }
  await waitFor(cdp, `(() => {
    const root = document.querySelector('#timelineContentScroll');
    const bounds = root.getBoundingClientRect();
    const visibleImages = [...root.querySelectorAll('.gallery-card img')].filter((image) => {
      const rect = image.getBoundingClientRect();
      return rect.bottom > bounds.top && rect.top < bounds.bottom;
    });
    return visibleImages.length > 0 && visibleImages.every((image) => image.complete && image.naturalWidth > 0);
  })()`,
    10000);
  result.settled = await evalValue(cdp, 'window.__galleryReadAnchor()');
  const naturalRatios = new Set();
  for (const card of result.settled.cards) {
    if (!card.image?.naturalWidth) continue;
    const naturalRatio = card.image.naturalHeight / card.image.naturalWidth;
    naturalRatios.add(naturalRatio);
    assert.ok(Math.abs(card.image.height / card.image.width - naturalRatio) <= 0.015,
      `[${viewportLabel}] ${card.id} 必须保持自然比例：${JSON.stringify(card.image)}`);
  }
  assert.ok(naturalRatios.size >= 2, `[${viewportLabel}] 应覆盖至少两种真实自然比例`);
  await evalValue(cdp, `(async () => {
    document.querySelector('#timelineContentScroll').scrollTop = 0;
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  })()`);
  result.top = await measureLayoutTop(cdp, 'image');
  assert.equal(result.top.footerInScrollerViewport, false, `[${viewportLabel}] 画廊异步测量后顶部不显示页脚`);
  result.bottom = await scrollLayoutToBottom(cdp);
  assert.equal(result.bottom.footerInScrollerViewport, true, `[${viewportLabel}] 画廊异步测量后底部页脚可见`);
  assert.equal(result.bottom.footerWithinScrollerBottom, true, `[${viewportLabel}] 画廊异步测量后页脚不越界`);
}

/** G1：真实解码混合比例深滚，挂载必须是逐列窗口并集，Tab不得回到屏外overscan。
    与上面的G2异步逐帧锚位分开验收；原≤3px断言保持原位置与原门槛。 */
async function runGalleryWindowCheck(cdp, evidence, viewportLabel) {
  await installLayoutFixture(cdp);
  const runs = [];
  evidence.galleryWindow = runs;
  for (const shape of ['mixed', 'uniform', 'flat']) {
    await evalValue(cdp, `(async () => {
      await window.__uiLayoutSelect('article');
      const store = window.__uiLayoutStore;
      const template = store.getState().entries.find((entry) => entry.feedId === 'f-nasa');
      const heights = ${JSON.stringify(shape)} === 'mixed' ? [300,800,1000,160]
        : ${JSON.stringify(shape)} === 'flat' ? [10] : [800];
      const width = ${JSON.stringify(shape)} === 'flat' ? 1800 : 600;
      const now = Date.now();
      store.setState({ entries: Array.from({ length: 5000 }, (_, index) => ({
        ...template, id: 'window-' + index, title: 'Gallery window ' + index,
        isRead: false, isStarred: false, publishedAt: now - index * 60000,
        imageUrl: 'data:image/svg+xml,' + encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="' + width + '" height="' + heights[index % heights.length] + '"><rect width="100%" height="100%" fill="#315b7d"/></svg>'),
      })), articlesExhausted: true });
      await window.__uiLayoutSelect('image');
    })()`);
    const samples = [];
    for (let phase = 0; phase < (shape === 'mixed' ? 3 : 1); phase++) {
      const sample = await evalValue(cdp, `(async () => {
        const { galleryMountBound } = await import('/src/components/GalleryLayout.ts');
        const root = document.querySelector('#timelineContentScroll');
        for (let frame=0;frame<180;frame++) {
          root.scrollTop += 45;
          await new Promise(requestAnimationFrame);
        }
        for(let frame=0;frame<12;frame++) await new Promise(requestAnimationFrame);
        const rect=root.getBoundingClientRect();
        const cards=[...root.querySelectorAll('.gallery-virtual-item')].map((element)=>{
          const bounds=element.getBoundingClientRect();const target=element.querySelector('[data-card-index]');
          return {index:Number(element.dataset.index),lane:Number(element.dataset.galleryLane),top:bounds.top-rect.top,bottom:bounds.bottom-rect.top,height:bounds.height,tab:target?.tabIndex,decoded:element.querySelector('img')?.naturalWidth>0};
        });
        const height=Math.min(...cards.map(card=>card.height).filter(height=>height>0));
        const lanes=new Set(cards.map(card=>card.lane)).size;
        return {scrollTop:root.scrollTop,mounted:cards.length,visible:cards.filter(card=>card.bottom>0&&card.top<root.clientHeight).length,
          bound:galleryMountBound(root.clientHeight,height,lanes),clientHeight:root.clientHeight,scrollWidth:root.scrollWidth,clientWidth:root.clientWidth,cards};
      })()`, 20000);
      samples.push(sample);
      assert.ok(sample.visible > 0, `[${viewportLabel}/${shape}] 深滚后必须有可见卡`);
      assert.ok(sample.cards.some((card) => card.decoded), `[${viewportLabel}/${shape}] 图片必须真实解码`);
      assert.ok(sample.mounted <= sample.bound,
        `[${viewportLabel}/${shape}] scroll=${sample.scrollTop} 挂载${sample.mounted}超过逐列界${sample.bound}`);
      assert.ok(sample.scrollWidth <= sample.clientWidth + 1, `[${viewportLabel}/${shape}] 不可横向溢出`);
      assert.equal(sample.cards.filter((card) => card.tab === 0).length, 1, `[${viewportLabel}/${shape}] 恰有一个Tab入口`);
    }
    await evalValue(cdp, `document.querySelector('.timeline-control-bar button[title="将当前列表全部标为已读"]').focus({preventScroll:true})`);
    const beforeTab = await evalValue(cdp, `(() => {
      const root=document.querySelector('#timelineContentScroll'),rect=root.getBoundingClientRect();
      const target=root.querySelector('[data-card-index][tabindex="0"]');
      const card=target?.closest('.gallery-card');
      const wrapper=target?.closest('.gallery-virtual-item');
      const bounds=wrapper?.getBoundingClientRect();
      return {
        scrollTop:root.scrollTop,
        clientHeight:root.clientHeight,
        target:target ? {
          id:card?.dataset.id,
          index:Number(target.dataset.cardIndex),
          top:bounds.top-rect.top,
          bottom:bounds.bottom-rect.top,
        } : null,
      };
    })()`);
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Tab', code: 'Tab', windowsVirtualKeyCode: 9 });
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Tab', code: 'Tab', windowsVirtualKeyCode: 9 });
    const keyboard = await evalValue(cdp, `(async()=>{
      for(let frame=0;frame<12;frame++) await new Promise(requestAnimationFrame);
      const root=document.querySelector('#timelineContentScroll'),rect=root.getBoundingClientRect();
      const focused=document.activeElement,card=focused?.closest('.gallery-card'),bounds=focused?.getBoundingClientRect();
      return {id:card?.dataset.id,index:Number(focused?.dataset.cardIndex),top:bounds?.top-rect.top,bottom:bounds?.bottom-rect.top,scrollTop:root.scrollTop,clientHeight:root.clientHeight};
    })()`);
    runs.push({ shape, samples, beforeTab, keyboard });
    assert.ok(keyboard.id, `[${viewportLabel}/${shape}] Tab须进入真实画廊卡`);
    assert.ok(keyboard.bottom > 0 && keyboard.top < keyboard.clientHeight, `[${viewportLabel}/${shape}] Tab最终焦点必须可见`);
    assert.ok(beforeTab.target, `[${viewportLabel}/${shape}] 按Tab前应有唯一画廊候选`);
    assert.ok(beforeTab.target.bottom > 0 && beforeTab.target.top < beforeTab.clientHeight,
      `[${viewportLabel}/${shape}] Tab候选必须已在视口内（top=${beforeTab.target.top}, bottom=${beforeTab.target.bottom}）`);
    assert.equal(keyboard.id, beforeTab.target.id,
      `[${viewportLabel}/${shape}] Tab必须落到按Tab前的可见候选，而非屏外历史卡`);
    assert.equal(keyboard.index, beforeTab.target.index,
      `[${viewportLabel}/${shape}] Tab焦点下标必须与候选一致`);
    const revealScroll = Math.max(0, -beforeTab.target.top, beforeTab.target.bottom - beforeTab.clientHeight);
    assert.ok(Math.abs(keyboard.scrollTop - beforeTab.scrollTop) <= revealScroll + 3,
      `[${viewportLabel}/${shape}] Tab只可为完整显示当前候选微调滚动：实际 ${keyboard.scrollTop - beforeTab.scrollTop}px，上限 ${Math.ceil(revealScroll + 3)}px`);
  }
  await screenshot(cdp, path.join(EVIDENCE_DIR, `${viewportLabel}-gallery-deep-window.png`));
}

async function pressBrowserKey(cdp, key, code, windowsVirtualKeyCode, text = undefined) {
  for (const type of ['keyDown', 'keyUp']) {
    await cdp.send('Input.dispatchKeyEvent', {
      type,
      key,
      code,
      windowsVirtualKeyCode,
      nativeVirtualKeyCode: windowsVirtualKeyCode,
      ...(type === 'keyDown' && text ? { text, unmodifiedText: text } : {}),
    });
  }
}

async function clickBrowserAt(cdp, x, y) {
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y });
  await cdp.send('Input.dispatchMouseEvent', {
    type: 'mousePressed',
    x,
    y,
    button: 'left',
    clickCount: 1,
  });
  await cdp.send('Input.dispatchMouseEvent', {
    type: 'mouseReleased',
    x,
    y,
    button: 'left',
    clickCount: 1,
  });
}

async function runKeyboardA11yChecks(cdp, evidence, viewportLabel) {
  const result = {};
  evidence.keyboardA11y = result;
  await cdp.send('Accessibility.enable');
  await cdp.send('DOM.enable');
  await installLayoutFixture(cdp);
  await evalValue(cdp, `(() => {
    const store = window.__uiLayoutStore;
    const s = store.getState();
    store.setState({
      activeContentLayout: 'article',
      activeArticleId: null,
      activeViewFilter: 'all',
      activeFeedFilter: 'all',
      timelineFilter: 'all',
      settings: { ...s.settings, listWidth: 560 },
    });
    return true;
  })()`);
  await delay(350);

  const geometry = await evalValue(cdp, `(() => {
    const rect = (selector) => {
      const node = document.querySelector(selector);
      if (!node) return null;
      const r = node.getBoundingClientRect();
      return { left: r.left, right: r.right, width: r.width, height: r.height };
    };
    return {
      innerWidth,
      innerHeight,
      scrollWidth: document.documentElement.scrollWidth,
      app: rect('.app-root'),
      sidebar: rect('.sidebar'),
      timeline: rect('.timeline-col'),
      reader: rect('.reader-col'),
    };
  })()`);
  result.responsiveGeometry = geometry;
  assert.ok(geometry.timeline?.width > 0 && geometry.reader?.width > 0,
    `[${viewportLabel}] 三栏应有真实布局宽度`);
  assert.ok(geometry.timeline.width <= 420.5,
    `[${viewportLabel}] 列表列宽应受 420px 上限约束，实际 ${geometry.timeline.width}`);
  if (geometry.innerWidth <= 1000) {
    assert.ok(geometry.timeline.width >= geometry.innerWidth * 0.34
      && geometry.timeline.width <= geometry.innerWidth * 0.40,
    `[${viewportLabel}] 窄窗列表应约为视口 34–40%，实际 ${geometry.timeline.width}/${geometry.innerWidth}`);
  }
  assert.ok(geometry.reader.width >= 340,
    `[${viewportLabel}] 阅读器应保留可读宽度，实际 ${geometry.reader.width}`);
  assert.ok(geometry.scrollWidth <= geometry.innerWidth + 1,
    `[${viewportLabel}] 页面不得横向溢出：scrollWidth=${geometry.scrollWidth}, innerWidth=${geometry.innerWidth}`);
  await screenshot(cdp, path.join(EVIDENCE_DIR, `${viewportLabel}-responsive-article.png`));

  const keyboard = {};
  for (const layout of LAYOUTS) {
    await evalValue(cdp, `window.__uiLayoutSelect(${JSON.stringify(layout)})`);
    await evalValue(cdp, `document.activeElement?.blur?.()`);
    const visibleIds = await evalValue(cdp, `window.__uiLayoutVisible().map((item) => item.id)`);
    assert.ok(visibleIds.length >= 2, `[${viewportLabel}/${layout}] J/K 夹具至少 2 条`);

    const snapshot = () => evalValue(cdp, `(() => {
      const active = document.activeElement;
      const owner = active?.closest?.('[data-id]') ?? active;
      const rect = owner?.getBoundingClientRect?.();
      return {
        activeArticleId: window.__uiLayoutStore.getState().activeArticleId,
        focusId: owner?.getAttribute?.('data-id') ?? null,
        focusTag: active?.tagName ?? null,
        ariaCurrent: active?.getAttribute?.('aria-current') ?? null,
        selectedIds: [...document.querySelectorAll('.active-selected')].map((node) => node.getAttribute('data-id')),
        focusInViewport: rect ? rect.top < innerHeight && rect.bottom > 0 : null,
        focusTop: rect?.top ?? null,
        focusBottom: rect?.bottom ?? null,
        scrollTop: document.querySelector('#timelineContentScroll')?.scrollTop ?? null,
        mountedIndexes: [...document.querySelectorAll('[data-card-index]')].map((node) => Number(node.getAttribute('data-card-index'))),
      };
    })()`);

    await pressBrowserKey(cdp, 'j', 'KeyJ', 74);
    await delay(180);
    const afterJ = await snapshot();
    assert.equal(afterJ.activeArticleId, visibleIds[0], `[${viewportLabel}/${layout}] J 应选中第一条`);
    assert.equal(afterJ.focusId, visibleIds[0], `[${viewportLabel}/${layout}] J 后 DOM 焦点应跟随`);
    assert.equal(afterJ.ariaCurrent, 'true', `[${viewportLabel}/${layout}] 焦点卡应带 aria-current=true`);
    assert.deepEqual(afterJ.selectedIds, [visibleIds[0]], `[${viewportLabel}/${layout}] 选中视觉应唯一落在目标卡`);
    assert.equal(afterJ.focusInViewport, true, `[${viewportLabel}/${layout}] 目标卡应滚动可见`);
    await screenshot(cdp, path.join(EVIDENCE_DIR, `${viewportLabel}-keyboard-j-${layout}.png`));

    await pressBrowserKey(cdp, 'k', 'KeyK', 75);
    await delay(180);
    const afterK = await snapshot();
    keyboard[layout] = { afterJ, afterK };
    result.keyboard = keyboard;
    assert.equal(afterK.activeArticleId, visibleIds.at(-1), `[${viewportLabel}/${layout}] K 应回绕到末条`);
    assert.equal(afterK.focusId, visibleIds.at(-1), `[${viewportLabel}/${layout}] K 后焦点应跟随末条`);
    assert.equal(afterK.focusInViewport, true, `[${viewportLabel}/${layout}] K 目标卡应滚动可见`);
    await screenshot(cdp, path.join(EVIDENCE_DIR, `${viewportLabel}-keyboard-k-${layout}.png`));
  }

  await evalValue(cdp, `window.__uiLayoutSelect('social')`);
  await evalValue(cdp, `(() => {
    window.__uiLayoutStore.setState({
      activeViewFilter: 'starred',
      timelineFilter: 'all',
      activeArticleId: null,
    });
    return true;
  })()`);
  await delay(300);
  const migrationBefore = await evalValue(cdp, `window.__uiLayoutVisible().map(({ id, title }) => ({ id, title }))`);
  assert.ok(migrationBefore.length >= 2, `[${viewportLabel}] 收藏筛选夹具至少 2 条`);
  await evalValue(cdp, `(() => {
    const card = document.querySelector('.social-card');
    const star = card?.querySelector('.social-act-item.starred');
    star?.focus();
    star?.click();
    return Boolean(star);
  })()`);
  await delay(300);
  const migrationAfter = await evalValue(cdp, `(() => {
    const active = document.activeElement;
    const owner = active?.closest?.('[data-id]') ?? active;
    return {
      focusId: owner?.getAttribute?.('data-id') ?? null,
      live: document.querySelector('[data-live-region="timeline"]')?.textContent ?? '',
      liveRole: document.querySelector('[data-live-region="timeline"]')?.getAttribute('role') ?? null,
      liveAriaLive: document.querySelector('[data-live-region="timeline"]')?.getAttribute('aria-live') ?? null,
      liveAriaAtomic: document.querySelector('[data-live-region="timeline"]')?.getAttribute('aria-atomic') ?? null,
      visibleIds: window.__uiLayoutVisible().map((item) => item.id),
    };
  })()`);
  assert.equal(migrationAfter.focusId, migrationBefore[1].id,
    `[${viewportLabel}] 取消收藏后焦点应迁到原下标下一卡`);
  assert.match(migrationAfter.live, /已取消收藏/,
    `[${viewportLabel}] live region 应宣告取消收藏`);
  assert.ok(migrationAfter.live.includes(migrationBefore[1].title),
    `[${viewportLabel}] live region 应点名新焦点卡`);
  assert.equal(migrationAfter.liveRole, 'status', `[${viewportLabel}] live region role 应为 status`);
  assert.equal(migrationAfter.liveAriaLive, 'polite', `[${viewportLabel}] live region 应为 polite`);
  assert.equal(migrationAfter.liveAriaAtomic, 'true', `[${viewportLabel}] live region 应为 atomic`);
  await screenshot(cdp, path.join(EVIDENCE_DIR, `${viewportLabel}-focus-migration.png`));

  await evalValue(cdp, `window.__uiLayoutSelect('image')`);
  await evalValue(cdp, `(() => {
    window.__uiLayoutStore.setState({
      activeViewFilter: 'starred',
      timelineFilter: 'all',
      activeArticleId: null,
    });
    return true;
  })()`);
  await delay(300);
  const galleryMigrationBefore = await evalValue(cdp, `window.__uiLayoutVisible().map(({ id, title }) => ({ id, title }))`);
  assert.ok(galleryMigrationBefore.length >= 2, `[${viewportLabel}] 画廊收藏筛选夹具至少 2 条`);
  await evalValue(cdp, `(() => {
    const star = document.querySelector('.gallery-card .gallery-meta button[title="取消收藏"]');
    star?.focus();
    star?.click();
    return Boolean(star);
  })()`);
  await delay(300);
  const galleryMigrationAfter = await evalValue(cdp, `(() => {
    const active = document.activeElement;
    const owner = active?.closest?.('[data-id]') ?? active;
    return {
      focusId: owner?.getAttribute?.('data-id') ?? null,
      live: document.querySelector('[data-live-region="timeline"]')?.textContent ?? '',
      liveRole: document.querySelector('[data-live-region="timeline"]')?.getAttribute('role') ?? null,
      liveAriaLive: document.querySelector('[data-live-region="timeline"]')?.getAttribute('aria-live') ?? null,
      liveAriaAtomic: document.querySelector('[data-live-region="timeline"]')?.getAttribute('aria-atomic') ?? null,
    };
  })()`);
  assert.equal(galleryMigrationAfter.focusId, galleryMigrationBefore[1].id,
    `[${viewportLabel}] 画廊取消收藏后焦点应迁到原线性顺序下一条`);
  assert.ok(galleryMigrationAfter.live.includes(galleryMigrationBefore[1].title),
    `[${viewportLabel}] 画廊焦点迁移也应由 live region 点名目标`);
  assert.equal(galleryMigrationAfter.liveRole, 'status', `[${viewportLabel}] 画廊 live region role 应为 status`);
  assert.equal(galleryMigrationAfter.liveAriaLive, 'polite', `[${viewportLabel}] 画廊 live region 应为 polite`);
  assert.equal(galleryMigrationAfter.liveAriaAtomic, 'true', `[${viewportLabel}] 画廊 live region 应为 atomic`);
  await screenshot(cdp, path.join(EVIDENCE_DIR, `${viewportLabel}-focus-migration-gallery.png`));

  const sidebarStructure = await evalValue(cdp, `(() => {
    const sidebar = document.querySelector('.sidebar');
    const nested = [];
    for (const button of sidebar?.querySelectorAll('button') ?? []) {
      if (button.parentElement?.closest('button, [role="button"]')) {
        nested.push(button.getAttribute('aria-label') || button.textContent?.trim() || button.className);
      }
    }
    const roleButtons = [...(sidebar?.querySelectorAll('[role="button"]') ?? [])]
      .map((node) => node.getAttribute('aria-label') || node.textContent?.trim() || node.className);
    return { nested, roleButtons };
  })()`);
  assert.deepEqual(sidebarStructure.nested, [],
    `[${viewportLabel}] Sidebar DOM 中不得出现 button 嵌套`);
  assert.deepEqual(sidebarStructure.roleButtons, [],
    `[${viewportLabel}] Sidebar 不再有 role=button 容器`);

  const sidebarObject = await cdp.send('Runtime.evaluate', {
    expression: `document.querySelector('.sidebar')`,
    returnByValue: false,
  });
  assert.ok(sidebarObject.result.objectId, `[${viewportLabel}] Sidebar 应可查询 AX 树`);
  const sidebarTree = await cdp.send('Accessibility.queryAXTree', {
    objectId: sidebarObject.result.objectId,
  });
  const axById = new Map(sidebarTree.nodes.map((node) => [node.nodeId, node]));
  const sidebarButtons = sidebarTree.nodes
    .filter((node) => !node.ignored && node.role?.value === 'button');
  const nestedAxButtons = sidebarButtons.filter((node) => {
    let parent = axById.get(node.parentId);
    while (parent) {
      if (parent.role?.value === 'button') return true;
      parent = axById.get(parent.parentId);
    }
    return false;
  }).map((node) => String(node.name?.value ?? '').trim());
  assert.deepEqual(nestedAxButtons, [],
    `[${viewportLabel}] AX 树中不得出现 button 父/子嵌套：${JSON.stringify(nestedAxButtons)}`);

  const sourceTabStart = await evalValue(cdp, `(() => {
    const row = document.querySelector('.feed-leaf-item[data-id]');
    const select = row?.querySelector('.feed-leaf-select');
    const actions = row?.querySelector('.feed-leaf-actions');
    select?.focus();
    return {
      rowId: row?.getAttribute('data-id') ?? null,
      tag: select?.tagName ?? null,
      visibility: actions ? getComputedStyle(actions).visibility : null,
      opacity: actions ? getComputedStyle(actions).opacity : null,
    };
  })()`);
  assert.ok(sourceTabStart.rowId, `[${viewportLabel}] 应存在订阅源行`);
  assert.equal(sourceTabStart.tag, 'BUTTON', `[${viewportLabel}] 源行主选择应为真实 button`);
  assert.equal(sourceTabStart.visibility, 'visible',
    `[${viewportLabel}] 焦点进入源行后操作按钮应可见`);

  const sourceTabOrder = [];
  for (let step = 0; step < 2; step += 1) {
    await pressBrowserKey(cdp, 'Tab', 'Tab', 9);
    await delay(90);
    sourceTabOrder.push(await evalValue(cdp, `(() => {
      const active = document.activeElement;
      return {
        tag: active?.tagName ?? null,
        action: active?.getAttribute?.('data-feed-action') ?? null,
        label: active?.getAttribute?.('aria-label') ?? null,
        visibility: getComputedStyle(document.querySelector('.feed-leaf-actions')).visibility,
      };
    })()`));
  }
  assert.deepEqual(sourceTabOrder.map((item) => item.action), ['refresh', 'edit'],
    `[${viewportLabel}] Tab 应从源行主选择依次到刷新、编辑`);
  assert.ok(sourceTabOrder.every((item) => item.visibility === 'visible'),
    `[${viewportLabel}] Tab 进入动作按钮时操作区应保持可见`);
  await screenshot(cdp, path.join(EVIDENCE_DIR, `${viewportLabel}-sidebar-actions-tab.png`));

  await evalValue(cdp, `(async () => {
    const row = document.querySelector('.feed-leaf-item[data-id]');
    row?.scrollIntoView({ block: 'center' });
    await new Promise((resolve) => requestAnimationFrame(resolve));
    return Boolean(row);
  })()`);
  const paddingTarget = await evalValue(cdp, `(() => {
    const row = document.querySelector('.feed-leaf-item[data-id]');
    const select = row?.querySelector('.feed-leaf-select');
    if (!row || !select) return null;
    const rowRect = row.getBoundingClientRect();
    const selectRect = select.getBoundingClientRect();
    return {
      id: row.getAttribute('data-id'),
      row: {
        left: rowRect.left,
        top: rowRect.top,
        bottom: rowRect.bottom,
        centerY: rowRect.top + rowRect.height / 2,
      },
      selectCenterX: selectRect.left + selectRect.width / 2,
    };
  })()`);
  assert.ok(paddingTarget?.id, `[${viewportLabel}] padding 点击探针需要订阅源行`);
  const paddingPoints = [
    {
      edge: 'left',
      x: paddingTarget.row.left + 5,
      y: paddingTarget.row.centerY,
    },
    {
      edge: 'top',
      x: paddingTarget.selectCenterX,
      y: paddingTarget.row.top + 1,
    },
    {
      edge: 'bottom',
      x: paddingTarget.selectCenterX,
      y: paddingTarget.row.bottom - 1,
    },
  ];
  const paddingClicks = [];
  for (const point of paddingPoints) {
    await evalValue(cdp, `window.__uiLayoutStore.setState({ activeFeedFilter: 'all' })`);
    await clickBrowserAt(cdp, point.x, point.y);
    await delay(120);
    const selected = await evalValue(cdp, `window.__uiLayoutStore.getState().activeFeedFilter`);
    paddingClicks.push({ ...point, selected });
    assert.equal(selected, paddingTarget.id,
      `[${viewportLabel}] 点击源行 ${point.edge} padding 应选中 ${paddingTarget.id}，实际 ${selected}`);
  }

  await evalValue(cdp, `(() => {
    const store = window.__uiLayoutStore;
    window.__uiLayoutRefreshCalls = [];
    const original = store.getState().refreshOneFeed;
    store.setState({
      refreshOneFeed: (id) => {
        window.__uiLayoutRefreshCalls.push(String(id));
        return original(id);
      },
    });
    return true;
  })()`);

  const playerProbe = await evalValue(cdp, `(() => {
    const store = window.__uiLayoutStore;
    const state = store.getState();
    store.setState({
      player: {
        ...state.player,
        isActive: true,
        isPlaying: false,
        title: 'Space probe',
        audioUrl: 'https://audio.example/probe.mp3',
      },
      playerExpanded: false,
    });
    return { feedId: document.querySelector('.feed-leaf-item[data-id]')?.getAttribute('data-id') ?? null };
  })()`);
  assert.ok(playerProbe.feedId, `[${viewportLabel}] Space 探针需要订阅源行`);

  await evalValue(cdp, `(() => {
    const button = document.querySelector('.sidebar-search-pill');
    button?.focus();
    return Boolean(button);
  })()`);
  await pressBrowserKey(cdp, ' ', 'Space', 32);
  await delay(220);
  const searchAfterSpace = await evalValue(cdp, `(() => ({
    searchOpen: window.__uiLayoutStore.getState().searchOpen,
    overlayOpen: Boolean(document.querySelector('.modal-overlay.open')),
    isPlaying: window.__uiLayoutStore.getState().player.isPlaying,
  }))()`);
  assert.equal(searchAfterSpace.searchOpen, true, `[${viewportLabel}] Sidebar 搜索按钮 Space 应打开搜索`);
  assert.equal(searchAfterSpace.overlayOpen, true, `[${viewportLabel}] 搜索浮层应真实打开`);
  assert.equal(searchAfterSpace.isPlaying, false,
    `[${viewportLabel}] 搜索按钮 Space 不得切换播放`);
  await pressBrowserKey(cdp, 'Escape', 'Escape', 27);
  await delay(180);

  const enterSelection = await evalValue(cdp, `(() => {
    const primary = document.querySelector('.feed-leaf-item[data-id] .feed-leaf-select');
    primary?.focus();
    return primary?.closest('.feed-leaf-item')?.getAttribute('data-id') ?? null;
  })()`);
  await pressBrowserKey(cdp, 'Enter', 'Enter', 13, '\r');
  await delay(180);
  const enterFeed = await evalValue(cdp, `window.__uiLayoutStore.getState().activeFeedFilter`);
  assert.equal(enterFeed, enterSelection, `[${viewportLabel}] 源行主选择 Enter 应触发选择`);

  const spaceSelection = await evalValue(cdp, `(() => {
    window.__uiLayoutStore.setState({ activeFeedFilter: 'all' });
    const primary = document.querySelector('.feed-leaf-item[data-id] .feed-leaf-select');
    primary?.focus();
    return primary?.closest('.feed-leaf-item')?.getAttribute('data-id') ?? null;
  })()`);
  await pressBrowserKey(cdp, ' ', 'Space', 32);
  await delay(220);
  const spaceFeed = await evalValue(cdp, `window.__uiLayoutStore.getState().activeFeedFilter`);
  assert.equal(spaceFeed, spaceSelection,
    `[${viewportLabel}] 源行主选择 Space 应触发选择且不切播放`);
  assert.equal(await evalValue(cdp, `window.__uiLayoutStore.getState().player.isPlaying`), false,
    `[${viewportLabel}] 源行主选择 Space 不得切换播放`);

  await evalValue(cdp, `(() => {
    document.querySelector('.feed-leaf-item[data-id] [data-feed-action="refresh"]')?.focus();
    return true;
  })()`);
  await pressBrowserKey(cdp, ' ', 'Space', 32);
  await delay(220);
  const refreshAfterSpace = await evalValue(cdp, `(() => ({
    calls: window.__uiLayoutRefreshCalls.slice(),
    isPlaying: window.__uiLayoutStore.getState().player.isPlaying,
  }))()`);
  assert.ok(refreshAfterSpace.calls.includes(String(spaceSelection)),
    `[${viewportLabel}] 刷新按钮 Space 应触发刷新自身`);
  assert.equal(refreshAfterSpace.isPlaying, false,
    `[${viewportLabel}] 刷新按钮 Space 不得切换播放`);

  await evalValue(cdp, `(() => {
    document.querySelector('.feed-leaf-item[data-id] [data-feed-action="edit"]')?.focus();
    return true;
  })()`);
  await pressBrowserKey(cdp, ' ', 'Space', 32);
  await delay(220);
  const editAfterSpace = await evalValue(cdp, `(() => ({
    editOpen: window.__uiLayoutStore.getState().editFeedModalOpen,
    isPlaying: window.__uiLayoutStore.getState().player.isPlaying,
  }))()`);
  assert.equal(editAfterSpace.editOpen, true,
    `[${viewportLabel}] 编辑按钮 Space 应打开编辑框`);
  assert.equal(editAfterSpace.isPlaying, false,
    `[${viewportLabel}] 编辑按钮 Space 不得切换播放`);
  await pressBrowserKey(cdp, 'Escape', 'Escape', 27);
  await delay(180);

  await evalValue(cdp, `window.__uiLayoutSelect('article')`);
  await pressBrowserKey(cdp, 'j', 'KeyJ', 74);
  await delay(180);
  await evalValue(cdp, `(() => {
    const proto = HTMLMediaElement.prototype;
    const originalPlay = proto.play;
    const originalPause = proto.pause;
    window.__uiLayoutRestoreMedia = () => {
      proto.play = originalPlay;
      proto.pause = originalPause;
    };
    proto.play = () => Promise.resolve();
    proto.pause = () => {};
    return true;
  })()`);
  await evalValue(cdp, `(() => {
    const store = window.__uiLayoutStore;
    const state = store.getState();
    store.setState({
      player: { ...state.player, isActive: true, isPlaying: false },
      playerExpanded: false,
    });
    return true;
  })()`);
  await pressBrowserKey(cdp, ' ', 'Space', 32);
  await delay(180);
  const cardSpace = await evalValue(cdp, `(() => ({
    isPlaying: window.__uiLayoutStore.getState().player.isPlaying,
    activeArticleId: window.__uiLayoutStore.getState().activeArticleId,
    playerActive: window.__uiLayoutStore.getState().player.isActive,
    focusTag: document.activeElement?.tagName ?? null,
    focusClass: document.activeElement?.className ?? null,
  }))()`);
  assert.match(String(cardSpace.focusClass), /article-card/,
    `[${viewportLabel}] 卡片 Space 探针应聚焦在文章卡`);
  assert.equal(cardSpace.isPlaying, true,
    `[${viewportLabel}] 卡片焦点上的 Space 仍应切换播放`);

  await evalValue(cdp, `(() => {
    window.__uiLayoutRestoreMedia?.();
    const store = window.__uiLayoutStore;
    const state = store.getState();
    store.setState({
      player: { ...state.player, isActive: false, isPlaying: false },
      activeFeedFilter: 'all',
    });
    return true;
  })()`);
  const feedSpace = spaceSelection;
  result.sidebarStructure = {
    domNestedButtons: sidebarStructure.nested,
    roleButtonRows: sidebarStructure.roleButtons,
    axButtonCount: sidebarButtons.length,
    axNestedButtons: nestedAxButtons,
  };
  result.sidebarPaddingClicks = paddingClicks;
  result.sidebarTabs = sourceTabOrder;
  result.sidebarSpace = {
    searchAfterSpace,
    feedSpace,
    enterSelection,
    enterFeed,
    spaceSelection,
    spaceFeed,
    refreshAfterSpace,
    editAfterSpace,
    cardSpace,
  };

  await evalValue(cdp, `window.__uiLayoutStore.getState().openSettingsTab('general')`);
  await delay(300);
  const settingsGeometry = await evalValue(cdp, `(() => {
    const rect = (selector) => {
      const node = document.querySelector(selector);
      if (!node) return null;
      const r = node.getBoundingClientRect();
      return { width: r.width, height: r.height, left: r.left, right: r.right };
    };
    return {
      innerWidth,
      modal: rect('.settings-modal'),
      sidebar: rect('.settings-sidebar'),
      pane: rect('.settings-content-pane'),
      bodyOverflow: getComputedStyle(document.body).overflow,
    };
  })()`);
  assert.ok(settingsGeometry.modal && settingsGeometry.modal.width <= settingsGeometry.innerWidth - 8,
    `[${viewportLabel}] 设置弹窗不得越过视口`);
  const expectedSettingsSidebar = settingsGeometry.innerWidth <= 1000 ? 170
    : settingsGeometry.innerWidth <= 1100 ? 190
      : 220;
  assert.ok(Math.abs(settingsGeometry.sidebar.width - expectedSettingsSidebar) < 1,
    `[${viewportLabel}] 设置侧栏断点宽度应为 ${expectedSettingsSidebar}，实际 ${settingsGeometry.sidebar.width}`);
  assert.ok(settingsGeometry.pane.width >= 720,
    `[${viewportLabel}] 设置内容区应保留可读宽度，实际 ${settingsGeometry.pane.width}`);
  result.settingsGeometry = settingsGeometry;

  const settingsAx = {};
  for (const tab of ['general', 'appearance', 'reading', 'feeds', 'ai', 'sync', 'shortcuts', 'about']) {
    await evalValue(cdp, `window.__uiLayoutStore.getState().switchSettingsTab(${JSON.stringify(tab)})`);
    await delay(220);
    const paneObject = await cdp.send('Runtime.evaluate', {
      expression: `document.querySelector('.modal-overlay.open .settings-content-pane')`,
      returnByValue: false,
    });
    const objectId = paneObject.result.objectId;
    assert.ok(objectId, `[${viewportLabel}/${tab}] 设置内容区应可查询 AX 树`);
    const tree = await cdp.send('Accessibility.queryAXTree', { objectId });
    const controls = [];
    for (const node of tree.nodes.filter((item) => !item.ignored
      && ['checkbox', 'slider', 'textbox', 'combobox', 'searchbox'].includes(item.role?.value))) {
      const role = node.role?.value;
      const name = String(node.name?.value ?? '').replace(/\s+/g, ' ').trim();
      let currentValue = String(node.value?.value ?? '').trim();
      if (role === 'combobox' && node.backendDOMNodeId != null) {
        const resolved = await cdp.send('DOM.resolveNode', { backendNodeId: node.backendDOMNodeId });
        const controlObjectId = resolved.object?.objectId;
        assert.ok(controlObjectId,
          `[${viewportLabel}/${tab}] combobox 应能解析到 DOM 节点以读取当前值`);
        const valueResult = await cdp.send('Runtime.callFunctionOn', {
          objectId: controlObjectId,
          functionDeclaration: `function () {
            const label = this.querySelector?.('.flux-dropdown-label');
            if (label) return (label.textContent ?? '').trim();
            if (this.tagName === 'SELECT') return (this.selectedOptions?.[0]?.textContent ?? '').trim();
            return (this.textContent ?? '').trim();
          }`,
          returnByValue: true,
        });
        const domValue = String(valueResult.result?.value ?? '').trim();
        if (domValue) currentValue = domValue;
      }
      controls.push({ role, name, currentValue });
    }
    for (const control of controls) {
      assert.ok(control.name,
        `[${viewportLabel}/${tab}] ${control.role} 在 AX 树中名称不得为空`);
      if (control.role === 'combobox') {
        assert.ok(control.currentValue,
          `[${viewportLabel}/${tab}] combobox 当前选项值不得为空`);
        assert.notEqual(control.name, control.currentValue,
          `[${viewportLabel}/${tab}] combobox 名称不得等于当前选项值「${control.currentValue}」`);
      }
    }
    if (['general', 'reading', 'ai', 'feeds'].includes(tab)) {
      assert.ok(controls.length > 0, `[${viewportLabel}/${tab}] 应有可检查的表单控件`);
    }
    settingsAx[tab] = controls;
  }
  result.keyboard = keyboard;
  result.focusMigration = {
    social: { before: migrationBefore, after: migrationAfter },
    gallery: { before: galleryMigrationBefore, after: galleryMigrationAfter },
  };
  result.settingsAx = settingsAx;
  result.settingsAxControlCount = Object.fromEntries(
    Object.entries(settingsAx).map(([tab, controls]) => [tab, controls.length]),
  );
  await evalValue(cdp, `window.__uiLayoutStore.getState().closeSettings()`);
  await delay(160);
}

const SOCIAL_KINDS = ['video', 'bodyimage', 'long', 'image', 'short', 'empty', 'imagefail'];
const SOCIAL_COLLAPSED_CARD_HEIGHT = 310;
const SOCIAL_COLLAPSED_ROW_HEIGHT = 322;
const SOCIAL_COLLAPSED_BODY_HEIGHT = 150;
const SOCIAL_EXPAND_SLOT_HEIGHT = 26;
const socialKind = (id, baseId) => SOCIAL_KINDS[(Number(id) - baseId) % SOCIAL_KINDS.length];
const hasExpandableContent = (card) =>
  (card.kind === 'long' && card.contentChars > 100)
  || ((card.kind === 'image' || card.kind === 'bodyimage') && card.imageHeight != null)
  || (card.kind === 'video' && (card.videoHeight ?? 0) > 0);

async function installSocialFixture(cdp, baseId, mediaBaseUrl) {
  await evalValue(cdp, `(async () => {
    const { useAppStore } = await import('/src/store.ts');
    const { api } = await import('/src/lib/api.ts');
    const now = Date.now();
    const kinds = ${JSON.stringify(SOCIAL_KINDS)};
    const baseId = ${baseId};
    const mediaBase = ${JSON.stringify(mediaBaseUrl)};
    const mediaUrl = (path, delay) => mediaBase + path + '?delay=' + delay + '&v=' + baseId;
    const svg = (width, height) => 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(
      '<svg xmlns="http://www.w3.org/2000/svg" width="' + width + '" height="' + height + '">' +
      '<rect width="100%" height="100%" fill="#4d6f93"/><text x="24" y="48" fill="white" font-size="28">IMAGE_BODY_MARKER</text></svg>'
    );
    const longBody = '<p>LONG_BODY_MARKER</p>' + Array.from({ length: 18 }, (_, index) =>
      '<p>第' + index + '段：18 段长文用于测量折叠上限与虚拟列表逐帧稳定性。</p>'
    ).join('');
    const imageBody = '<p>IMAGE_BODY_MARKER 图片正文</p><img src="' + svg(800, 1100) + '" alt="" />';
    const shortBody = '<p>SHORT_BODY_MARKER 短文正文天然低于折叠上限。</p>';
    const bodyImageBody = '<p>BODY_IMAGE_MARKER 行无封面但正文含 HTTP 图</p><img src="'
      + mediaUrl('/delayed-image.svg', 900)
      + '" alt="" />';
    const failedImageBody = '<p>FAILED_IMAGE_MARKER 图片加载失败</p><img src="'
      + mediaUrl('/failed-image.svg', 500)
      + '" alt="" />';
    /* poster 延迟到视频 metadata 之后返回；metadata 先用真实 176×144 尺寸
       触发内层增长，随后 poster 再把视觉高度抬到其自然比例。 */
    const videoBody = '<p>VIDEO_BODY_MARKER 视频 poster 延迟加载</p><video src="'
      + mediaUrl('/video.mp4', 900)
      + '" poster="'
      + mediaUrl('/poster.svg', 1600)
      + '" preload="metadata" controls muted playsinline></video>';
    const emptyBody = '';
    const kindOf = (id) => kinds[(Number(id) - baseId) % kinds.length];
    const bodyOf = (kind) => kind === 'long'
      ? longBody
      : kind === 'image'
        ? imageBody
        : kind === 'bodyimage'
          ? bodyImageBody
          : kind === 'imagefail'
            ? failedImageBody
            : kind === 'video'
              ? videoBody
          : kind === 'empty'
            ? emptyBody
            : shortBody;
    const entries = Array.from({ length: 35 }, (_, index) => {
      const kind = kinds[index % kinds.length];
      return {
        id: String(baseId + index),
        feedId: 'f-sama',
        title: '社交 ' + kind + ' 条目 ' + String(index + 1).padStart(2, '0'),
        snippet: kind === 'image' || kind === 'long'
          ? '长摘要用于估算折叠正文区，避免所有社交卡都从固定 320px 起步。'.repeat(6)
          : '短摘要：正文是否含图与折叠外高无关。',
        imageUrl: kind === 'image' ? svg(800, 1100) : null,
        publishedAt: now - index * 60_000,
        source: 'direct',
        isRead: index % 2 === 1,
        isStarred: true,
        tags: [],
        author: 'Sam Altman',
        content: '',
        rawContent: '',
        aiSummary: '',
        translatedContent: '',
      };
    });
    window.__socialAudit = {
      running: true,
      startedAt: 0,
      installAt: performance.now(),
      samples: [],
      shifts: [],
      styleMutations: [],
      mediaEvents: [],
      apiCalls: { getArticles: 0, getArticlesStartedAt: null, getArticlesResolvedAt: null },
    };
    api.getArticles = async (ids) => {
      window.__socialAudit.apiCalls.getArticles += 1;
      window.__socialAudit.apiCalls.getArticlesStartedAt = performance.now();
      await new Promise((resolve) => setTimeout(resolve, ${HYDRATE_DELAY_MS}));
      window.__socialAudit.apiCalls.getArticlesResolvedAt = performance.now();
      return ids.map((id) => ({
        id,
        feed_id: -1,
        title: '社交条目',
        author: 'Sam Altman',
        snippet: 'snippet',
        image_url: null,
        enclosure_url: null,
        enclosure_mime: null,
        duration_sec: null,
        ai_summary: null,
        source: 'direct',
        published_at: new Date(now).toISOString(),
        is_read: false,
        is_starred: true,
        url: 'https://example.invalid/article/' + id,
        content_html: bodyOf(kindOf(id)),
        translated_content: '',
        fulltext_extracted: false,
        content_id: id,
        content_revision: 1,
      }));
    };
    const rectOf = (rect) => rect ? {
      x: Math.round(rect.x),
      y: Math.round(rect.y),
      width: Math.round(rect.width),
      height: Math.round(rect.height),
    } : null;
    const centerInsideSocialBody = (rect) => {
      if (!rect || !rect.width || !rect.height) return false;
      const centerX = rect.x + rect.width / 2;
      const centerY = rect.y + rect.height / 2;
      return [...document.querySelectorAll('.social-text-inner')].some((node) => {
        const bodyRect = node.getBoundingClientRect();
        return centerX >= bodyRect.left && centerX <= bodyRect.right
          && centerY >= bodyRect.top && centerY <= bodyRect.bottom;
      });
    };
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        const sources = entry.sources?.map((source) => {
          const previousRect = rectOf(source.previousRect);
          const currentRect = rectOf(source.currentRect);
          return {
            node: source.node ? (source.node.className || source.node.nodeName) : null,
            insideSocialBody: source.node instanceof Element
              ? Boolean(source.node.closest('.social-text-inner'))
              : centerInsideSocialBody(currentRect) || centerInsideSocialBody(previousRect),
            previousRect,
            currentRect,
          };
        }) ?? [];
        window.__socialAudit.shifts.push({
          t: Math.round(entry.startTime - window.__socialAudit.installAt),
          startTime: entry.startTime,
          value: entry.value,
          hadRecentInput: entry.hadRecentInput,
          sources,
        });
      }
    }).observe({ type: 'layout-shift', buffered: false });
    new MutationObserver((records) => {
      const t = Math.round(performance.now() - window.__socialAudit.startedAt);
      for (const record of records) {
        if (record.type !== 'attributes' || record.attributeName !== 'style') continue;
        const target = record.target;
        if (!(target instanceof HTMLElement)) continue;
        if (!target.classList.contains('timeline-virtual-item')
          && !target.classList.contains('timeline-virtual-wrap')) continue;
        if (window.__socialAudit.styleMutations.length >= 5000) return;
        window.__socialAudit.styleMutations.push({
          t,
          type: target.className,
          index: target.dataset.index ?? null,
          transform: target.style.transform,
          height: target.style.height,
          offsetHeight: target.offsetHeight,
        });
      }
    }).observe(document.body, {
      subtree: true,
      attributes: true,
      attributeFilter: ['style'],
      childList: true,
    });

    const mediaSrcScheme = (src) => !src
      ? null
      : src.startsWith('data:')
        ? 'data'
        : /^https?:/.test(src)
          ? 'http'
          : 'other';
    const mediaSrcPath = (src) => {
      if (!src) return null;
      try { return new URL(src, location.href).pathname; } catch { return null; }
    };
    const recordMediaEvent = (event) => {
      const target = event.target;
      if (!(target instanceof HTMLImageElement) && !(target instanceof HTMLVideoElement)) return;
      const src = target.currentSrc || target.src || '';
      const poster = target instanceof HTMLVideoElement ? target.poster : '';
      window.__socialAudit.mediaEvents.push({
        t: Math.round(performance.now() - window.__socialAudit.startedAt),
        type: event.type,
        tag: target.tagName,
        srcScheme: mediaSrcScheme(src),
        srcPath: mediaSrcPath(src),
        posterScheme: mediaSrcScheme(poster),
        posterPath: mediaSrcPath(poster),
        complete: target instanceof HTMLImageElement ? target.complete : null,
        naturalWidth: target instanceof HTMLImageElement ? target.naturalWidth : null,
        naturalHeight: target instanceof HTMLImageElement ? target.naturalHeight : null,
        readyState: target instanceof HTMLVideoElement ? target.readyState : null,
        videoWidth: target instanceof HTMLVideoElement ? target.videoWidth : null,
        videoHeight: target instanceof HTMLVideoElement ? target.videoHeight : null,
      });
    };
    recordMediaEvent.__socialAuditRecorder = true;
    const mediaEventNames = ['load', 'error', 'loadedmetadata', 'loadeddata', 'durationchange'];
    const recordedMediaNodes = new WeakSet();
    const attachMediaRecorder = (node) => {
      if (!(node instanceof HTMLImageElement) && !(node instanceof HTMLVideoElement)) return;
      if (recordedMediaNodes.has(node)) return;
      recordedMediaNodes.add(node);
      for (const eventName of mediaEventNames) node.addEventListener(eventName, recordMediaEvent);
    };
    const scanMediaRecorder = (root = document) => {
      if (root instanceof HTMLImageElement || root instanceof HTMLVideoElement) attachMediaRecorder(root);
      if (!(root instanceof Document) && !(root instanceof Element)) return;
      for (const node of root.querySelectorAll('.social-text-inner img, .social-text-inner video')) {
        attachMediaRecorder(node);
      }
    };
    scanMediaRecorder();
    const mediaRecorderObserver = new MutationObserver((records) => {
      for (const record of records) {
        for (const node of record.addedNodes) scanMediaRecorder(node);
      }
    });
    mediaRecorderObserver.observe(document.body, { childList: true, subtree: true });
    for (const eventName of mediaEventNames) {
      document.addEventListener(eventName, recordMediaEvent, true);
    }

    let mediaNodeSeq = 0;
    const mediaNodeIds = new WeakMap();
    const mediaNodeId = (node) => {
      if (!node) return null;
      let id = mediaNodeIds.get(node);
      if (!id) {
        id = ++mediaNodeSeq;
        mediaNodeIds.set(node, id);
      }
      return id;
    };

    const inspectCard = (outer) => {
      const inner = outer.firstElementChild;
      const text = inner?.querySelector('.social-text');
      const textInner = text?.querySelector('.social-text-inner');
      const textRect = text?.getBoundingClientRect();
      const textInnerRect = textInner?.getBoundingClientRect();
      const image = text?.querySelector('img');
      const imageRect = image?.getBoundingClientRect();
      const video = text?.querySelector('video');
      const videoRect = video?.getBoundingClientRect();
      const button = inner?.querySelector('.social-expand-btn');
      const slot = inner?.querySelector('.social-expand-slot');
      const slotRect = slot?.getBoundingClientRect();
      const computed = text ? getComputedStyle(text) : null;
      const rect = outer.getBoundingClientRect();
      const innerRect = inner?.getBoundingClientRect();
      const visibleImageHeight = textRect && imageRect
        ? Math.max(0, Math.min(textRect.bottom, imageRect.bottom) - Math.max(textRect.top, imageRect.top))
        : null;
      const visibleVideoHeight = textRect && videoRect
        ? Math.max(0, Math.min(textRect.bottom, videoRect.bottom) - Math.max(textRect.top, videoRect.top))
        : null;
      const imageSrc = image ? (image.currentSrc || image.src) : null;
      const videoSrc = video ? (video.currentSrc || video.src) : null;
      const videoPoster = video?.poster || null;
      return {
        id: inner?.dataset.id ?? null,
        index: Number(outer.dataset.index),
        virtualStart: Number(outer.dataset.virtualStart),
        virtualSize: Number(outer.dataset.virtualSize),
        top: rect.top,
        bottom: rect.bottom,
        height: rect.height,
        cardHeight: innerRect?.height ?? null,
        textHeight: textRect?.height ?? null,
        textClientHeight: text?.clientHeight ?? null,
        textScrollHeight: text?.scrollHeight ?? null,
        textClientWidth: text?.clientWidth ?? null,
        textScrollWidth: text?.scrollWidth ?? null,
        textClass: text?.className ?? null,
        textMaxHeight: computed?.maxHeight ?? null,
        textOverflowY: computed?.overflowY ?? null,
        textMinHeight: text?.style.minHeight ?? null,
        textStyleHeight: text?.style.height ?? null,
        cardStyleHeight: inner?.style.height ?? null,
        innerHeight: textInnerRect?.height ?? null,
        contentChars: text?.textContent?.length ?? 0,
        placeholder: inner?.querySelector('.hydrate-placeholder')?.textContent ?? null,
        imageSrcScheme: mediaSrcScheme(imageSrc),
        imageSrcPath: mediaSrcPath(imageSrc),
        imageComplete: image?.complete ?? null,
        imageNaturalWidth: image?.naturalWidth ?? null,
        imageNaturalHeight: image?.naturalHeight ?? null,
        imageHeight: imageRect?.height ?? null,
        visibleImageHeight,
        visibleVideoHeight,
        videoSrcScheme: mediaSrcScheme(videoSrc),
        videoPosterScheme: mediaSrcScheme(videoPoster),
        videoSrcPath: mediaSrcPath(videoSrc),
        videoPosterPath: mediaSrcPath(videoPoster),
        videoNodeId: mediaNodeId(video),
        videoReadyState: video?.readyState ?? null,
        videoWidth: video?.videoWidth ?? null,
        videoHeight: video?.videoHeight ?? null,
        videoRectHeight: videoRect?.height ?? null,
        cardClientWidth: inner?.clientWidth ?? null,
        cardScrollWidth: inner?.scrollWidth ?? null,
        slotHeight: slotRect?.height ?? null,
        hasExpandButton: Boolean(button),
        buttonAriaExpanded: button?.getAttribute('aria-expanded') ?? null,
        buttonAriaControls: button?.getAttribute('aria-controls') ?? null,
      };
    };

    window.__socialAudit.capturePlaceholder = () => [...document.querySelectorAll('.timeline-virtual-item')]
      .map(inspectCard);
    const snapshot = () => {
      const scroller = document.querySelector('#timelineContentScroll');
      const cards = [...document.querySelectorAll('.timeline-virtual-item')].map(inspectCard);
      let overlapCount = 0;
      const ordered = [...cards].sort((left, right) => left.top - right.top);
      for (let index = 1; index < ordered.length; index++) {
        if (ordered[index].top < ordered[index - 1].bottom - 1) overlapCount += 1;
      }
      return {
        t: performance.now() - window.__socialAudit.startedAt,
        scrollTop: scroller?.scrollTop ?? null,
        scrollHeight: scroller?.scrollHeight ?? null,
        cards,
        overlapCount,
        horizontalOverflowCount: cards.filter((card) =>
          (card.textScrollWidth != null && card.textClientWidth != null
            && card.textScrollWidth > card.textClientWidth + 1)
          || (card.cardScrollWidth != null && card.cardClientWidth != null
            && card.cardScrollWidth > card.cardClientWidth + 1)
        ).length,
        slotHeightViolations: cards.filter((card) =>
          card.slotHeight == null || Math.abs(card.slotHeight - ${SOCIAL_EXPAND_SLOT_HEIGHT}) > 0.5
        ).length,
      };
    };
    const sample = () => {
      window.__socialAudit.samples.push(snapshot());
    };
    const getVirtualItem = (index) => document.querySelector(
      '.timeline-virtual-item[data-index="' + index + '"]'
    );
    const scrollToIndex = async (index) => {
      const scroller = document.querySelector('#timelineContentScroll');
      scroller.scrollTop = Math.max(0, index * ${SOCIAL_COLLAPSED_ROW_HEIGHT});
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      await new Promise((resolve) => setTimeout(resolve, 80));
      return getVirtualItem(index);
    };
    window.__socialAudit.snapshot = snapshot;
    window.__socialAudit.captureCardAt = async (index) => {
      const item = await scrollToIndex(index);
      if (!item) return null;
      scanMediaRecorder(item);
      const scroller = document.querySelector('#timelineContentScroll');
      return {
        card: inspectCard(item),
        scrollTop: scroller.scrollTop,
        scrollHeight: scroller.scrollHeight,
      };
    };
    window.__socialAudit.captureCurrentCardAt = (index) => {
      const item = getVirtualItem(index);
      if (!item) return null;
      scanMediaRecorder(item);
      return { card: inspectCard(item) };
    };
    window.__socialAudit.runExpandCollapse = async (index) => {
      const scroller = document.querySelector('#timelineContentScroll');
      const item = await scrollToIndex(index);
      if (!item) throw new Error('目标虚拟项未挂载: ' + index);
      /* 视频在滚动/虚拟项复用后可能重新触发 poster/metadata；先等目标视频
         回到可测量状态，并等 poster 高度连续两次取样稳定，避免展开动作
         与迟到的媒体解码叠在同一段逐帧采样里。 */
      if (item.querySelector('.social-text-inner video')) {
        const deadline = performance.now() + 15000;
        let stableVideoHeight = null;
        let settled = false;
        while (performance.now() < deadline) {
          const current = getVirtualItem(index);
          const currentVideo = current?.querySelector('.social-text-inner video');
          const videoHeight = currentVideo?.getBoundingClientRect().height ?? 0;
          if (currentVideo?.readyState >= 1
            && current?.querySelector('.social-expand-btn')
            && videoHeight > 500
            && stableVideoHeight != null
            && Math.abs(videoHeight - stableVideoHeight) < 0.5) {
            settled = true;
            break;
          }
          stableVideoHeight = videoHeight > 500 ? videoHeight : null;
          await new Promise((resolve) => setTimeout(resolve, 50));
        }
        if (!settled) {
          const current = getVirtualItem(index);
          const currentVideo = current?.querySelector('.social-text-inner video');
          throw new Error(
            '视频在展开前未完成 poster/元数据布局: readyState=' + (currentVideo?.readyState ?? 'null')
            + ', height=' + (currentVideo?.getBoundingClientRect().height ?? 'null')
          );
        }
      }
      const card = item.querySelector('.social-card');
      if (!card) throw new Error('目标社交卡未挂载: ' + index);
      const id = card.dataset.id;
      const cardState = () => {
        const current = getVirtualItem(index);
        return current ? inspectCard(current) : null;
      };
      const findButton = () => {
        const current = getVirtualItem(index);
        return current?.querySelector('.social-expand-btn') ?? null;
      };
      const before = cardState();
      const scrollTopBefore = scroller.scrollTop;
      const scrollHeightBefore = scroller.scrollHeight;
      const frames = [];
      let running = true;
      const recordFrame = () => {
        const frame = snapshot();
        frames.push({
          t: frame.t,
          scrollTop: frame.scrollTop,
          scrollHeight: frame.scrollHeight,
          overlapCount: frame.overlapCount,
          horizontalOverflowCount: frame.horizontalOverflowCount,
          slotHeightViolations: frame.slotHeightViolations,
          target: cardState(),
          cards: frame.overlapCount > 0
            ? frame.cards.map((card) => ({
              id: card.id,
              index: card.index,
              top: card.top,
              bottom: card.bottom,
              height: card.height,
              textClass: card.textClass,
              virtualStart: card.virtualStart,
              virtualSize: card.virtualSize,
            }))
            : [],
        });
      };
      recordFrame();
      const loop = () => {
        if (!running) return;
        /* rAF 回调处于本帧渲染步进之前：异步二次高度变化（展开后仍会到达的
           字体/重排）在该帧 RO 回调里已被测量+坐标直写收敛，paint 前完成，
           用户不可见；在 rAF 相位采样会记录一个从未上屏的中间态。延到渲染
           步进结束（setTimeout(0)）再采样，在本帧 RO 收敛后采样，过滤相位中间态；重叠阈值仍为 0。
           Note: 采样语义与坐标单一写入者 — 见 .agents/notes/implemented/bug-fix/2026-11-07-社交虚拟行坐标单一写入者与动作采样对齐paint.md */
        requestAnimationFrame(() => setTimeout(() => {
          if (!running) return;
          recordFrame();
          loop();
        }, 0));
      };
      loop();
      const expandButton = findButton();
      if (!expandButton) throw new Error('目标卡缺少展开按钮: ' + id);
      expandButton.click();
      await new Promise((resolve) => setTimeout(resolve, 240));
      const expanded = cardState();
      const scrollTopExpanded = scroller.scrollTop;
      const scrollHeightExpanded = scroller.scrollHeight;
      const collapseButton = findButton();
      if (!collapseButton) throw new Error('展开后缺少收起按钮: ' + id);
      collapseButton.click();
      await new Promise((resolve) => setTimeout(resolve, 240));
      running = false;
      await new Promise((resolve) => setTimeout(resolve, 48));
      return {
        index,
        id,
        scrollTopBefore,
        scrollTopExpanded,
        scrollTopAfter: scroller.scrollTop,
        scrollHeightBefore,
        scrollHeightExpanded,
        scrollHeightAfter: scroller.scrollHeight,
        before,
        expanded,
        after: cardState(),
        frames,
      };
    };
    window.__socialAudit.replaceBody = async (id, kind) => {
      const { getBodyEntry, applyBodyRow } = await import('/src/store/bodyCache.ts');
      const key = String(id);
      const current = getBodyEntry(key);
      applyBodyRow(
        key,
        {
          content_html: bodyOf(kind),
          translated_content: current?.translatedContent ?? '',
          ai_summary: current?.aiSummary ?? '',
        },
        current?.contentRevision ?? 0,
        {
          translatedContent: current?.translatedContent ?? '',
          aiSummary: current?.aiSummary ?? '',
        },
      );
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      await new Promise((resolve) => setTimeout(resolve, 120));
      const card = document.querySelector('.social-card[data-id="' + key + '"]');
      const item = card?.closest('.timeline-virtual-item');
      return item ? inspectCard(item) : null;
    };

    useAppStore.setState({
      dataMode: 'tauri',
      activeContentLayout: 'social',
      activeViewFilter: 'starred',
      activeFeedFilter: 'all',
      timelineFilter: 'all',
      timelineSort: 'newest',
      entries,
      articlesExhausted: true,
      articlesLoading: false,
      activeArticleId: null,
    });
    window.__socialAudit.startedAt = performance.now();
    const tick = () => {
      sample();
      if (performance.now() - window.__socialAudit.startedAt < ${SAMPLE_MS}) requestAnimationFrame(tick);
      else window.__socialAudit.running = false;
    };
    requestAnimationFrame(tick);
  })()`);
}

async function runSocialCheck(cdp, evidence, viewportLabel, baseId, media) {
  await installSocialFixture(cdp, baseId, media.url);
  await delay(140);
  const placeholder = await evalValue(cdp, `({
    cards: window.__socialAudit.capturePlaceholder(),
  })`);
  await screenshot(cdp, path.join(EVIDENCE_DIR, `${viewportLabel}-social-hydration-placeholder.png`));
  await waitFor(cdp, `(() => {
    const item = document.querySelector('.timeline-virtual-item[data-index="0"]');
    const video = item?.querySelector('.social-text-inner video');
    return !!item && item.textContent.includes('VIDEO_BODY_MARKER')
      && video?.readyState === 0;
  })()`, HYDRATE_DELAY_MS + 5000);
  const videoInitial = await evalValue(cdp, `window.__socialAudit.captureCurrentCardAt(0)?.card ?? null`);
  await screenshot(cdp, path.join(EVIDENCE_DIR, `${viewportLabel}-social-video-initial.png`));
  await waitFor(cdp, `(() => {
    const item = document.querySelector('.timeline-virtual-item[data-index="0"]');
    const video = item?.querySelector('.social-text-inner video');
    return video?.readyState >= 1;
  })()`, HYDRATE_DELAY_MS + 10000);
  await evalValue(cdp, `new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(() => setTimeout(resolve, 80)),
  ))`);
  const videoMetadata = await evalValue(cdp, `window.__socialAudit.captureCurrentCardAt(0)?.card ?? null`);
  await screenshot(cdp, path.join(EVIDENCE_DIR, `${viewportLabel}-social-video-metadata.png`));
  await waitFor(cdp, `(() => {
    const item = document.querySelector('.timeline-virtual-item[data-index="0"]');
    const video = item?.querySelector('.social-text-inner video');
    return video?.readyState >= 1 && video.getBoundingClientRect().height > 500;
  })()`, 15000);
  await evalValue(cdp, `new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(() => setTimeout(resolve, 80)),
  ))`);
  const videoPoster = await evalValue(cdp, `window.__socialAudit.captureCurrentCardAt(0)?.card ?? null`);
  await screenshot(cdp, path.join(EVIDENCE_DIR, `${viewportLabel}-social-video-poster.png`));
  await waitFor(cdp, 'window.__socialAudit && window.__socialAudit.running === false', SAMPLE_MS + 5000);
  /* 采样窗口恰好在某张视频卡 poster 到达的同帧结束时，mutation 后的 rAF
     重新测量可能还没跑。先等所有已出元数据的视频卡完成 poster 布局，再给
     一帧测量窗口取最终快照；这里不要求按钮已经出现，否则判红会落在等待
     超时而绕过 no-social-media-remeasure 的视频专项断言。 */
  await waitFor(cdp, `(() => {
    const cards = [...document.querySelectorAll('.social-card')];
    return cards.every((card) => {
      if (!card.textContent.includes('VIDEO_BODY_MARKER')) return true;
      const video = card.querySelector('.social-text-inner video');
      return !video || video.readyState === 0 || video.error != null
        || video.getBoundingClientRect().height > 500;
    });
  })()`, 5000);
  await evalValue(cdp, `new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(() => setTimeout(resolve, 80)),
  ))`);
  const finalSnapshot = await evalValue(cdp, `window.__socialAudit.snapshot()`);
  await screenshot(cdp, path.join(EVIDENCE_DIR, `${viewportLabel}-social-hydration-settled.png`));

  const social = await evalValue(cdp, `(() => ({
    installAt: window.__socialAudit.installAt,
    samples: window.__socialAudit.samples,
    shifts: window.__socialAudit.shifts,
    styleMutations: window.__socialAudit.styleMutations,
    mediaEvents: window.__socialAudit.mediaEvents,
    apiCalls: window.__socialAudit.apiCalls,
  }))()`);
  const variantCaptures = {};
  const variants = [
    ['bodyimage', 1, `(() => {
      const item = document.querySelector('.timeline-virtual-item[data-index="1"]');
      const image = item?.querySelector('.social-text-inner img');
      return !!item && image?.complete === true && image.naturalWidth > 0;
    })()`],
    ['short', 4, `(() => {
      const item = document.querySelector('.timeline-virtual-item[data-index="4"]');
      return !!item && item.textContent.includes('SHORT_BODY_MARKER');
    })()`],
    ['empty', 5, `(() => {
      const item = document.querySelector('.timeline-virtual-item[data-index="5"]');
      return !!item && item.textContent.includes('暂无正文');
    })()`],
    ['imagefail', 6, `(() => {
      const item = document.querySelector('.timeline-virtual-item[data-index="6"]');
      const image = item?.querySelector('.social-text-inner img');
      return !!item && image?.complete === true && image.naturalWidth === 0;
    })()`],
    ['video', 0, `(() => {
      const item = document.querySelector('.timeline-virtual-item[data-index="0"]');
      const video = item?.querySelector('.social-text-inner video');
      return !!item && ((video?.readyState >= 1
        && video.getBoundingClientRect().height > 500) || video?.error != null);
    })()`],
  ];
  for (const [name, index, readyExpression] of variants) {
    await evalValue(cdp, `window.__socialAudit.captureCardAt(${index})`);
    await waitFor(cdp, readyExpression, HYDRATE_DELAY_MS + 10000);
    variantCaptures[name] = await evalValue(cdp, `window.__socialAudit.captureCurrentCardAt(${index})`);
    await screenshot(cdp, path.join(EVIDENCE_DIR, `${viewportLabel}-social-collapsed-${name}.png`));
  }
  const actionCaptures = [];
  if (MUTATION !== 'no-social-media-remeasure') {
    await evalValue(cdp, `window.__socialAudit.replaceBody(${baseId + 4}, 'long')`);
    await waitFor(cdp, `(() => {
      const item = document.querySelector('.timeline-virtual-item[data-index="4"]');
      return !!item && item.textContent.includes('LONG_BODY_MARKER')
        && !!item.querySelector('.social-expand-btn');
    })()`, 2000);
    variantCaptures.replacedLong = await evalValue(cdp, `window.__socialAudit.captureCardAt(4)`);
    await evalValue(cdp, `window.__socialAudit.replaceBody(${baseId + 4}, 'short')`);
    await waitFor(cdp, `(() => {
      const item = document.querySelector('.timeline-virtual-item[data-index="4"]');
      return !!item && item.textContent.includes('SHORT_BODY_MARKER')
        && !item.querySelector('.social-expand-btn');
    })()`, 2000);
    variantCaptures.replacedShort = await evalValue(cdp, `window.__socialAudit.captureCardAt(4)`);
    for (const [name, index] of [['bodyimage', 1], ['long', 2], ['video', 0]]) {
      actionCaptures.push({
        name,
        result: await evalValue(cdp, `window.__socialAudit.runExpandCollapse(${index})`),
      });
    }
  }
  const mediaAfterActions = await evalValue(cdp, `({
    mediaEvents: window.__socialAudit.mediaEvents,
  })`);
  Object.assign(social, mediaAfterActions);
  const allCards = social.samples.flatMap((sample) => sample.cards.map((card) => ({
    ...card,
    kind: card.id ? socialKind(card.id, baseId) : 'unknown',
  })));
  const cardsByKind = Object.fromEntries(SOCIAL_KINDS.map((kind) => {
    const cards = allCards.filter((card) => card.kind === kind);
    const heights = cards.map((card) => card.height);
    const textHeights = cards.map((card) => card.textHeight).filter((height) => typeof height === 'number');
    const textClientHeights = cards.map((card) => card.textClientHeight).filter((height) => typeof height === 'number');
    const visibleImageHeights = cards.map((card) => card.visibleImageHeight).filter((height) => typeof height === 'number');
    return [kind, {
      samples: cards.length,
      minCardHeight: heights.length ? Math.min(...heights) : null,
      maxCardHeight: heights.length ? Math.max(...heights) : null,
      cardHeightSpread: heights.length ? Math.max(...heights) - Math.min(...heights) : null,
      maxTextHeight: textHeights.length ? Math.max(...textHeights) : null,
      maxTextClientHeight: textClientHeights.length ? Math.max(...textClientHeights) : null,
      maxVisibleImageHeight: visibleImageHeights.length ? Math.max(...visibleImageHeights) : null,
    }];
  }));
  const cardHeightsById = new Map();
  const cardStartsById = new Map();
  for (const card of allCards) {
    if (!card.id) continue;
    const heights = cardHeightsById.get(card.id) ?? [];
    heights.push(card.height);
    cardHeightsById.set(card.id, heights);
    const starts = cardStartsById.get(card.id) ?? [];
    starts.push(card.virtualStart);
    cardStartsById.set(card.id, starts);
  }
  const cardHeightSpreadById = [...cardHeightsById.values()].map((heights) =>
    Math.max(...heights) - Math.min(...heights)
  );
  const maxCardHeightSpreadById = cardHeightSpreadById.length
    ? Math.max(...cardHeightSpreadById)
    : null;
  const cardVirtualStartSpreadById = [...cardStartsById.values()].map((starts) =>
    Math.max(...starts) - Math.min(...starts)
  );
  const maxCardVirtualStartSpreadById = cardVirtualStartSpreadById.length
    ? Math.max(...cardVirtualStartSpreadById)
    : null;
  const placeholderCards = placeholder.cards;
  const placeholderHeightViolations = placeholderCards.filter((card) =>
    card.height == null || Math.abs(card.height - SOCIAL_COLLAPSED_ROW_HEIGHT) > 0.5
  ).length;
  const placeholderInnerHeightViolations = placeholderCards.filter((card) =>
    card.cardHeight == null || Math.abs(card.cardHeight - SOCIAL_COLLAPSED_CARD_HEIGHT) > 0.5
  ).length;
  const placeholderTextHeightViolations = placeholderCards.filter((card) =>
    card.textHeight == null || Math.abs(card.textHeight - SOCIAL_COLLAPSED_BODY_HEIGHT) > 0.5
  ).length;
  const placeholderStyleHeightViolations = placeholderCards.filter((card) =>
    card.cardStyleHeight !== `${SOCIAL_COLLAPSED_CARD_HEIGHT}px`
    || card.textStyleHeight !== `${SOCIAL_COLLAPSED_BODY_HEIGHT}px`
  ).length;
  const placeholderSlotHeightViolations = placeholderCards.filter((card) =>
    card.slotHeight == null || Math.abs(card.slotHeight - SOCIAL_EXPAND_SLOT_HEIGHT) > 0.5
  ).length;
  const placeholderLoadingMissing = placeholderCards.filter((card) =>
    !String(card.placeholder).includes('加载正文…')
  ).length;
  const placeholderHorizontalOverflowFrames = placeholderCards.filter((card) =>
    (card.textScrollWidth != null && card.textClientWidth != null
      && card.textScrollWidth > card.textClientWidth + 1)
    || (card.cardScrollWidth != null && card.cardClientWidth != null
      && card.cardScrollWidth > card.cardClientWidth + 1)
  ).length;
  const overlapFrames = social.samples.filter((sample) => sample.overlapCount > 0).length;
  const maxOverlapCount = social.samples.reduce((max, sample) => Math.max(max, sample.overlapCount), 0);
  const horizontalOverflowFrames = social.samples.filter((sample) =>
    sample.horizontalOverflowCount > 0
  ).length;
  const maxHorizontalOverflowCount = social.samples.reduce((max, sample) =>
    Math.max(max, sample.horizontalOverflowCount), 0);
  const slotHeightViolationFrames = social.samples.filter((sample) =>
    sample.slotHeightViolations > 0
  ).length;
  const overlapSamples = social.samples
    .filter((sample) => sample.overlapCount > 0)
    .map((sample) => ({
      t: Math.round(sample.t),
      overlapCount: sample.overlapCount,
      scrollHeight: sample.scrollHeight,
      cards: sample.cards.map((card) => ({
        id: card.id,
        kind: card.id ? socialKind(card.id, baseId) : 'unknown',
        top: Math.round(card.top),
        bottom: Math.round(card.bottom),
        height: Math.round(card.height),
        virtualStart: Math.round(card.virtualStart),
        virtualSize: Math.round(card.virtualSize),
        textClass: card.textClass,
        contentChars: card.contentChars,
      })),
    }));
  const longUncollapsedFrames = allCards.filter((card) =>
    card.contentChars > 100 && !String(card.textClass).includes('collapsed')
  ).length;
  const expandedFrames = allCards.filter((card) => card.buttonAriaExpanded === 'true').length;
  const expandableContentFrames = allCards.filter(hasExpandableContent);
  const expandButtonFrames = expandableContentFrames.filter((card) => card.hasExpandButton);
  const missingAriaExpanded = expandButtonFrames.filter((card) =>
    card.buttonAriaExpanded !== 'false'
  ).length;
  const missingAriaControls = expandButtonFrames.filter((card) =>
    card.buttonAriaControls !== `social-text-${card.id}`
  ).length;
  const finalCards = (finalSnapshot?.cards ?? []).map((card) => ({
    ...card,
    kind: card.id ? socialKind(card.id, baseId) : 'unknown',
  }));
  const finalExpandableCards = finalCards.filter(hasExpandableContent);
  const finalCardsMissingButton = finalExpandableCards.filter((card) => !card.hasExpandButton).length;
  const finalCardsMissingAria = finalExpandableCards.filter((card) =>
    card.buttonAriaExpanded !== 'false'
  ).length;
  const finalCardHeights = finalCards.map((card) => card.height);
  const finalCardHeightSpread = finalCardHeights.length
    ? Math.max(...finalCardHeights) - Math.min(...finalCardHeights)
    : null;
  const finalCardsHorizontalOverflow = finalCards.filter((card) =>
    (card.textScrollWidth != null && card.textClientWidth != null
      && card.textScrollWidth > card.textClientWidth + 1)
    || (card.cardScrollWidth != null && card.cardClientWidth != null
      && card.cardScrollWidth > card.cardClientWidth + 1)
  ).length;
  const finalCardsMissingAriaControls = finalExpandableCards.filter((card) =>
    card.hasExpandButton && card.buttonAriaControls !== `social-text-${card.id}`
  ).length;
  const imageFrames = allCards.filter((card) =>
    (card.kind === 'image' || card.kind === 'bodyimage') && card.imageHeight != null
  );
  const videoFrames = allCards.filter((card) =>
    card.kind === 'video' && card.videoRectHeight != null
  );
  const videoInnerHeights = videoFrames
    .map((card) => card.innerHeight)
    .filter((height) => typeof height === 'number');
  const videoRectHeights = videoFrames
    .map((card) => card.videoRectHeight)
    .filter((height) => typeof height === 'number');
  const videoInnerHeightSpread = videoInnerHeights.length
    ? Math.max(...videoInnerHeights) - Math.min(...videoInnerHeights)
    : null;
  const videoRectHeightSpread = videoRectHeights.length
    ? Math.max(...videoRectHeights) - Math.min(...videoRectHeights)
    : null;
  const videoBoundaryViolations = videoFrames.filter((card) =>
    card.textOverflowY !== 'hidden'
    || card.textMaxHeight !== '150px'
    || card.visibleVideoHeight == null
    || card.textHeight == null
    || card.visibleVideoHeight > card.textHeight + 1
  ).length;
  const imageBoundaryViolations = imageFrames.filter((card) =>
    card.textOverflowY !== 'hidden'
    || card.textMaxHeight !== '150px'
    || card.visibleImageHeight == null
    || card.textHeight == null
    || card.visibleImageHeight > card.textHeight + 1
  ).length;
  const scrollHeights = social.samples.map((sample) => sample.scrollHeight).filter((height) => typeof height === 'number');
  const scrollHeightSpread = scrollHeights.length ? Math.max(...scrollHeights) - Math.min(...scrollHeights) : null;
  const cumulativeLayoutShift = social.shifts.reduce((sum, entry) => sum + entry.value, 0);
  const maxLayoutShift = social.shifts.reduce((max, entry) => Math.max(max, entry.value), 0);
  const isMediaInnerShift = (entry) => entry.sources.length > 0
    && entry.sources.every((source) => source.insideSocialBody);
  const mediaInnerLayoutShift = social.shifts
    .filter(isMediaInnerShift)
    .reduce((sum, entry) => sum + entry.value, 0);
  const cumulativeLayoutShiftOutsideSocialBody = social.shifts
    .filter((entry) => !isMediaInnerShift(entry))
    .reduce((sum, entry) => sum + entry.value, 0);
  const maxLayoutShiftOutsideSocialBody = social.shifts
    .filter((entry) => !isMediaInnerShift(entry))
    .reduce((max, entry) => Math.max(max, entry.value), 0);
  const variantCards = Object.fromEntries(Object.entries(variantCaptures).map(([name, capture]) => [
    name,
    capture?.card ?? null,
  ]));
  const actionSummary = actionCaptures.map(({ name, result }) => ({
    name,
    index: result.index,
    id: result.id,
    scrollTopBefore: result.scrollTopBefore,
    scrollTopExpanded: result.scrollTopExpanded,
    scrollTopAfter: result.scrollTopAfter,
    scrollHeightBefore: result.scrollHeightBefore,
    scrollHeightExpanded: result.scrollHeightExpanded,
    scrollHeightAfter: result.scrollHeightAfter,
    frameCount: result.frames.length,
    overlapFrames: result.frames.filter((frame) => frame.overlapCount > 0).length,
    maxOverlapCount: result.frames.reduce((max, frame) =>
      Math.max(max, frame.overlapCount), 0),
    horizontalOverflowFrames: result.frames.filter((frame) =>
      frame.horizontalOverflowCount > 0
    ).length,
    slotHeightViolationFrames: result.frames.filter((frame) =>
      frame.slotHeightViolations > 0
    ).length,
    overlapSamples: result.frames.filter((frame) => frame.overlapCount > 0)
      .slice(0, 8)
      .map((frame) => ({
        t: Math.round(frame.t),
        scrollTop: frame.scrollTop,
        scrollHeight: frame.scrollHeight,
        overlapCount: frame.overlapCount,
        target: frame.target,
        cards: frame.cards,
      })),
    videoFrameStates: name === 'video'
      ? result.frames.map((frame) => ({
        t: Math.round(frame.t),
        overlapCount: frame.overlapCount,
        scrollHeight: frame.scrollHeight,
        target: frame.target ? {
          videoNodeId: frame.target.videoNodeId,
          videoReadyState: frame.target.videoReadyState,
          videoRectHeight: frame.target.videoRectHeight,
          innerHeight: frame.target.innerHeight,
          height: frame.target.height,
          virtualSize: frame.target.virtualSize,
          buttonAriaExpanded: frame.target.buttonAriaExpanded,
        } : null,
      }))
      : undefined,
    expandedFrames: result.frames.filter((frame) =>
      frame.target?.buttonAriaExpanded === 'true'
    ).length,
    lastFrame: result.frames.at(-1)?.target ?? null,
    before: result.before,
    expanded: result.expanded,
    after: result.after,
  }));
  const mediaRequests = media.requests.filter((request) =>
    request.path.includes(`v=${baseId}`)
  );
  const delayedImageRequests = mediaRequests.filter((request) =>
    request.path.includes('/delayed-image.svg')
  );
  const failedImageRequests = mediaRequests.filter((request) =>
    request.path.includes('/failed-image.svg')
  );
  const posterRequests = mediaRequests.filter((request) =>
    request.path.includes('/poster.svg')
  );
  const videoRequests = mediaRequests.filter((request) =>
    request.path.includes('/video.mp4')
  );
  const imageLoadEvents = social.mediaEvents.filter((event) =>
    event.type === 'load' && event.srcPath === '/delayed-image.svg'
  );
  const imageErrorEvents = social.mediaEvents.filter((event) =>
    event.type === 'error' && event.srcPath === '/failed-image.svg'
  );
  const videoMetadataEvents = social.mediaEvents.filter((event) =>
    event.tag === 'VIDEO' && (event.type === 'loadedmetadata' || event.type === 'loadeddata')
  );
  const placeholderByKind = Object.fromEntries(SOCIAL_KINDS.map((kind) => {
    const cards = placeholder.cards.filter((card) => card.id && socialKind(card.id, baseId) === kind);
    return [kind, cards];
  }));
  const result = {
    hydrateDelayMs: HYDRATE_DELAY_MS,
    sampleMs: SAMPLE_MS,
    installAt: social.installAt,
    placeholderByKind,
    videoInitial,
    videoMetadata,
    videoPoster,
    apiCalls: social.apiCalls,
    frameCount: social.samples.length,
    cardsByKind,
    cardHeightSpreadById: maxCardHeightSpreadById,
    cardVirtualStartSpreadById: maxCardVirtualStartSpreadById,
    finalCardHeightSpread,
    placeholderCardCount: placeholderCards.length,
    placeholderHeightViolations,
    placeholderInnerHeightViolations,
    placeholderTextHeightViolations,
    placeholderStyleHeightViolations,
    placeholderSlotHeightViolations,
    placeholderLoadingMissing,
    placeholderHorizontalOverflowFrames,
    longUncollapsedFrames,
    expandedFrames,
    expandableContentFrames: expandableContentFrames.length,
    expandButtonFrames: expandButtonFrames.length,
    missingAriaExpanded,
    missingAriaControls,
    finalExpandableCards: finalExpandableCards.length,
    finalCardsMissingButton,
    finalCardsMissingAria,
    finalCardsMissingAriaControls,
    finalCardsHorizontalOverflow,
    finalCards: finalCards.map((card) => ({
      id: card.id,
      kind: card.kind,
      height: card.height,
      textHeight: card.textHeight,
      textClass: card.textClass,
      contentChars: card.contentChars,
      imageHeight: card.imageHeight,
      videoRectHeight: card.videoRectHeight,
      videoReadyState: card.videoReadyState,
      hasExpandButton: card.hasExpandButton,
      buttonAriaExpanded: card.buttonAriaExpanded,
    })),
    imageFrames: imageFrames.length,
    imageBoundaryViolations,
    videoFrames: videoFrames.length,
    videoBoundaryViolations,
    videoFrameSamples: videoFrames.filter((card) =>
      card.textOverflowY !== 'hidden'
      || card.textMaxHeight !== '150px'
      || card.visibleVideoHeight == null
      || card.textHeight == null
      || card.visibleVideoHeight > card.textHeight + 1
    ).slice(0, 12).map((card) => ({
      id: card.id,
      index: card.index,
      top: card.top,
      textHeight: card.textHeight,
      textClass: card.textClass,
      textMaxHeight: card.textMaxHeight,
      textOverflowY: card.textOverflowY,
      innerHeight: card.innerHeight,
      videoRectHeight: card.videoRectHeight,
      visibleVideoHeight: card.visibleVideoHeight,
      videoReadyState: card.videoReadyState,
      hasExpandButton: card.hasExpandButton,
    })),
    videoInnerHeightSpread,
    videoRectHeightSpread,
    overlapFrames,
    maxOverlapCount,
    overlapSamples,
    horizontalOverflowFrames,
    maxHorizontalOverflowCount,
    slotHeightViolationFrames,
    scrollHeightSpread,
    scrollHeightSamples: social.samples.map((sample) => ({
      t: Math.round(sample.t),
      scrollHeight: sample.scrollHeight,
    })),
    styleMutations: social.styleMutations,
    mediaEvents: social.mediaEvents,
    mediaRequests,
    mediaEvidence: {
      delayedImageRequests,
      failedImageRequests,
      posterRequests,
      videoRequests,
      imageLoadEvents,
      imageErrorEvents,
      videoMetadataEvents,
    },
    cumulativeLayoutShift,
    maxLayoutShift,
    mediaInnerLayoutShift,
    cumulativeLayoutShiftOutsideSocialBody,
    maxLayoutShiftOutsideSocialBody,
    layoutShifts: social.shifts,
    variants: variantCards,
    actions: actionSummary,
  };
  evidence.socialHydration = result;

  assert.ok(result.apiCalls.getArticles >= 1, '[social] 延迟水合应真实发起 get_articles');
  assert.ok(result.frameCount > 20, `[social] 逐帧采样不足：${result.frameCount}`);
  assert.ok(result.placeholderCardCount > 0, '[social] 加载占位样本缺失');
  assert.equal(result.placeholderHeightViolations, 0,
    `[social] Loading 态整行外高必须固定 ${SOCIAL_COLLAPSED_ROW_HEIGHT}px；violations=${result.placeholderHeightViolations}`);
  assert.equal(result.placeholderInnerHeightViolations, 0,
    `[social] Loading 态卡片外高必须固定 ${SOCIAL_COLLAPSED_CARD_HEIGHT}px；violations=${result.placeholderInnerHeightViolations}`);
  assert.equal(result.placeholderTextHeightViolations, 0,
    `[social] Loading 态正文区必须固定 ${SOCIAL_COLLAPSED_BODY_HEIGHT}px；violations=${result.placeholderTextHeightViolations}`);
  assert.equal(result.placeholderStyleHeightViolations, 0,
    `[social] Loading 态必须使用固定内联外高/正文高；violations=${result.placeholderStyleHeightViolations}`);
  assert.equal(result.placeholderSlotHeightViolations, 0,
    `[social] Loading 态按钮槽高必须恒为 ${SOCIAL_EXPAND_SLOT_HEIGHT}px；violations=${result.placeholderSlotHeightViolations}`);
  assert.equal(result.placeholderLoadingMissing, 0,
    '[social] Loading 态必须保留「加载正文…」占位文案');
  assert.equal(result.placeholderHorizontalOverflowFrames, 0,
    '[social] Loading 态不得出现横向溢出');
  assert.ok(delayedImageRequests.length > 0,
    '[social] 正式 fixture 必须请求延迟 HTTP 图片');
  assert.ok(delayedImageRequests.every((request) => request.status === 200 && request.bytes > 100),
    '[social] 延迟 HTTP 图片必须成功返回图片字节');
  assert.ok(delayedImageRequests.some((request) =>
    request.finishedAt - request.startedAt >= 800
  ), '[social] HTTP 图片必须真实延迟返回，而不是 data URI 即时可用');
  assert.ok(imageLoadEvents.length > 0,
    '[social] HTTP 图片加载完成必须触发 load 事件证据');
  assert.ok(failedImageRequests.length > 0 && imageErrorEvents.length > 0,
    '[social] 必须覆盖 HTTP 图片失败与 error 事件');
  assert.ok(posterRequests.length > 0,
    '[social] 必须覆盖视频 poster 的 HTTP 请求');
  assert.ok(videoRequests.length > 0,
    '[social] 必须覆盖视频源的真实 HTTP 请求');
  assert.ok(videoRequests.some((request) => request.status === 206 && request.bytes > 100),
    '[social] 视频源必须以 Range/206 返回真实字节');
  assert.ok(posterRequests.some((request) => request.status === 200 && request.bytes > 100),
    '[social] 视频 poster 必须以 200 返回图片字节');
  assert.ok(result.imageFrames > 0, '[social] 图片正文样本缺失');
  assert.equal(result.imageBoundaryViolations, 0,
    `[social] 图片正文必须被 max-height/overflow 裁在正文上限内；violations=${result.imageBoundaryViolations}`);
  assert.equal(result.videoBoundaryViolations, 0,
    `[social] 视频正文必须被 max-height/overflow 裁在正文上限内；violations=${result.videoBoundaryViolations}`);
  assert.ok((cardsByKind.image.maxCardHeight ?? Infinity) < 500,
    `[social] 图片卡高度不得被图片撑开，实际 ${cardsByKind.image.maxCardHeight}`);
  assert.ok((cardsByKind.bodyimage.maxCardHeight ?? Infinity) < 500,
    `[social] 行无 imageUrl、正文含图的卡片高度不得被图片撑开，实际 ${cardsByKind.bodyimage.maxCardHeight}`);
  assert.ok((result.cardHeightSpreadById ?? Infinity) < 1,
    `[social] 同一卡片跨占位/水合帧的外高必须稳定，实际最大 spread=${result.cardHeightSpreadById}`);
  assert.ok((result.cardVirtualStartSpreadById ?? Infinity) < 1,
    `[social] 同一卡片跨占位/水合帧的虚拟起始位置必须稳定，实际最大 spread=${result.cardVirtualStartSpreadById}`);
  assert.ok((result.finalCardHeightSpread ?? Infinity) < 1,
    `[social] 所有落地社交卡的折叠外高必须统一，实际 spread=${result.finalCardHeightSpread}`);
  assert.equal(result.longUncollapsedFrames, 0,
    '[social] 所有正文首帧与后续帧都必须保持 collapsed，直至用户主动展开');
  assert.equal(result.expandedFrames, 0, '[social] 无用户操作时不得出现 aria-expanded=true');
  assert.ok(result.expandableContentFrames > 0, '[social] 可展开内容样本缺失');
  assert.ok(result.expandButtonFrames > 0, '[social] 可展开内容的展开按钮样本缺失');
  assert.equal(result.missingAriaExpanded, 0,
    '[social] 长文/图片正文的展开按钮必须暴露 aria-expanded=false');
  assert.equal(result.missingAriaControls, 0,
    `[social] 所有展开按钮必须用 aria-controls 指向对应正文；missing=${result.missingAriaControls}`);
  assert.ok(result.finalExpandableCards > 0, '[social] 落地帧缺少可展开内容样本');
  /* 这组视频专项断言放在通用 missing 计数之前：no-social-media-remeasure
     变异必须由视频路径本身判红，不能被其他可展开卡片的计数掩盖。 */
  assert.equal(variantCards.video?.hasExpandButton, true,
    `[social] 视频 metadata 后必须重新测量并出现展开按钮；readyState=${variantCards.video?.videoReadyState}`);
  assert.ok((variantCards.video?.innerHeight ?? 0) > (videoInitial?.innerHeight ?? 0) + 1,
    `[social] 视频 metadata 后内层高度必须增长：before=${videoInitial?.innerHeight}, after=${variantCards.video?.innerHeight}`);
  assert.equal(result.finalCardsMissingButton, 0,
    `[social] 落地帧的可展开卡片必须渲染展开按钮；missing=${result.finalCardsMissingButton}`);
  assert.equal(result.finalCardsMissingAria, 0,
    `[social] 落地帧的展开按钮必须暴露 aria-expanded=false；missing=${result.finalCardsMissingAria}`);
  assert.equal(result.finalCardsMissingAriaControls, 0,
    `[social] 展开按钮必须用 aria-controls 指向对应正文；missing=${result.finalCardsMissingAriaControls}`);
  assert.equal(result.finalCardsHorizontalOverflow, 0,
    `[social] 落地卡不得出现横向溢出；violations=${result.finalCardsHorizontalOverflow}`);
  assert.ok((cardsByKind.long.cardHeightSpread ?? Infinity) < 1,
    `[social] 18 段长文占位→落地高度应稳定，实际 spread=${cardsByKind.long.cardHeightSpread}`);
  assert.ok((cardsByKind.image.cardHeightSpread ?? Infinity) < 1,
    `[social] 图片正文占位→落地高度应稳定，实际 spread=${cardsByKind.image.cardHeightSpread}`);
  assert.ok((cardsByKind.bodyimage.cardHeightSpread ?? Infinity) < 1,
    `[social] 行无 imageUrl、正文含图的卡片占位→落地高度应稳定，实际 spread=${cardsByKind.bodyimage.cardHeightSpread}`);
  assert.ok((cardsByKind.short.cardHeightSpread ?? Infinity) < 1,
    `[social] 短文占位→落地高度应稳定，实际 spread=${cardsByKind.short.cardHeightSpread}`);
  assert.ok((cardsByKind.empty.cardHeightSpread ?? Infinity) < 1,
    `[social] 空正文占位→落地高度应稳定，实际 spread=${cardsByKind.empty.cardHeightSpread}`);
  assert.ok((cardsByKind.imagefail.cardHeightSpread ?? Infinity) < 1,
    `[social] 失败图片卡占位→落地高度应稳定，实际 spread=${cardsByKind.imagefail.cardHeightSpread}`);
  assert.ok((cardsByKind.video.cardHeightSpread ?? Infinity) < 1,
    `[social] 视频卡占位→落地高度应稳定，实际 spread=${cardsByKind.video.cardHeightSpread}`);
  assert.ok((cardsByKind.video.maxCardHeight ?? Infinity) < 500,
    `[social] 视频卡片高度不得被媒体撑开，实际 ${cardsByKind.video.maxCardHeight}`);
  assert.ok((cardsByKind.long.maxCardHeight ?? Infinity) < 500,
    `[social] 18 段长文卡片高度不得超出折叠上限，实际 ${cardsByKind.long.maxCardHeight}`);
  assert.ok((cardsByKind.short.maxTextHeight ?? 0) > 149
    && (cardsByKind.short.maxTextHeight ?? Infinity) < 151,
  `[social] 短文正文区必须保持统一 ${SOCIAL_COLLAPSED_BODY_HEIGHT}px，实际 ${cardsByKind.short.maxTextHeight}`);
  assert.equal(result.horizontalOverflowFrames, 0,
    `[social] 社交卡不得横向溢出；maxCount=${result.maxHorizontalOverflowCount}`);
  assert.equal(result.slotHeightViolationFrames, 0,
    '[social] 展开按钮槽高必须在所有帧保持 26px');
  assert.equal(result.overlapFrames, 0,
    `[social] 社交卡不得重叠；maxOverlapCount=${maxOverlapCount}`);
  assert.ok(scrollHeightSpread != null && scrollHeightSpread < 1,
    `[social] 占位估算不得造成大幅滚动总高跳变，实际 spread=${scrollHeightSpread}`);
  /* 裁切正文内的图片/视频自然增长是 fixture 要覆盖的媒体路径，Chromium 会
     为此记 layout-shift；卡片外框与相邻项不动已由 spread/overlap 断言锁住。
     这里只禁止影响正文容器之外的 shift，原始 CLS 仍完整留在 metrics。 */
  assert.ok(cumulativeLayoutShiftOutsideSocialBody < 0.01,
    `[social] 水合期间裁切正文外不得产生 CLS；outside=${cumulativeLayoutShiftOutsideSocialBody}, raw=${cumulativeLayoutShift}`);
  assert.ok(maxLayoutShiftOutsideSocialBody < 0.01,
    `[social] 水合期间裁切正文外单次 layout shift 必须为 0；maxOutside=${maxLayoutShiftOutsideSocialBody}, rawMax=${maxLayoutShift}`);

  assert.ok(videoInitial, '[social] 视频初始状态样本缺失');
  assert.equal(videoInitial.videoReadyState, 0,
    `[social] 视频初始 readyState 必须为 0，实际 ${videoInitial.videoReadyState}`);
  assert.equal(videoInitial.videoWidth, 0,
    `[social] 视频初始 natural width 必须为 0，实际 ${videoInitial.videoWidth}`);
  assert.equal(videoInitial.videoHeight, 0,
    `[social] 视频初始 natural height 必须为 0，实际 ${videoInitial.videoHeight}`);
  assert.equal(videoInitial.hasExpandButton, false,
    '[social] metadata 前视频卡不应提前出现展开按钮');
  assert.doesNotMatch(String(videoInitial.textClass), /social-text-clipped/,
    '[social] metadata 前视频卡不应提前出现裁切渐变');
  assert.ok(videoMetadata, '[social] 视频 metadata 中间态样本缺失');
  assert.ok(videoMetadata.videoReadyState >= 1,
    `[social] 视频 metadata 中间态 readyState 必须至少为 1，实际 ${videoMetadata.videoReadyState}`);
  assert.equal(videoMetadata.videoWidth, 176,
    `[social] 视频 metadata 自然宽度必须为 176，实际 ${videoMetadata.videoWidth}`);
  assert.equal(videoMetadata.videoHeight, 144,
    `[social] 视频 metadata 自然高度必须为 144，实际 ${videoMetadata.videoHeight}`);
  /* Chromium 在 poster 仍 pending 时把 video 的渲染盒保持为 0 高，metadata
     只更新 videoWidth/Height，不改变布局；高度增长/按钮/渐变由 poster 到达
     触发。因此 metadata 断言只锁自然尺寸，poster 态再锁布局与交互。 */
  assert.ok(videoPoster, '[social] 视频 poster 落地样本缺失');
  assert.ok(videoPoster.videoReadyState >= 1,
    `[social] 视频 poster 态 readyState 必须至少为 1，实际 ${videoPoster.videoReadyState}`);
  assert.equal(videoPoster.videoNodeId, videoMetadata.videoNodeId,
    '[social] metadata→poster 期间视频节点不得重建');
  assert.ok(videoPoster.innerHeight > videoMetadata.innerHeight + 1,
    `[social] 视频 poster 到达后内层自然高度必须增长：metadata=${videoMetadata.innerHeight}, poster=${videoPoster.innerHeight}`);
  assert.ok(videoPoster.videoRectHeight > videoMetadata.videoRectHeight + 1,
    `[social] 视频 poster 到达后视频高度必须增长：metadata=${videoMetadata.videoRectHeight}, poster=${videoPoster.videoRectHeight}`);
  assert.equal(videoPoster.hasExpandButton, true,
    '[social] 视频 poster 到达后必须出现展开按钮');
  assert.match(String(videoPoster.textClass), /social-text-clipped/,
    '[social] 视频 poster 到达后必须出现裁切渐变');
  assert.equal(videoPoster.buttonAriaExpanded, 'false',
    '[social] 视频 poster 到达后按钮必须暴露 aria-expanded=false');
  assert.equal(videoPoster.buttonAriaControls, `social-text-${videoPoster.id}`,
    '[social] 视频 poster 到达后按钮必须用 aria-controls 指向正文');
  assert.equal(variantCards.video.videoReadyState >= 1, true,
    '[social] metadata 后视频 readyState 必须至少为 1');
  assert.ok(variantCards.video.innerHeight > videoInitial.innerHeight + 1,
    `[social] metadata 后内层自然高度必须增长：before=${videoInitial.innerHeight}, after=${variantCards.video.innerHeight}`);
  assert.ok(variantCards.video.videoRectHeight > videoInitial.videoRectHeight + 1,
    `[social] metadata 后视频自然高度必须增长：before=${videoInitial.videoRectHeight}, after=${variantCards.video.videoRectHeight}`);
  assert.equal(variantCards.video.hasExpandButton, true,
    '[social] metadata 后视频卡必须出现展开按钮');
  assert.match(String(variantCards.video.textClass), /social-text-clipped/,
    '[social] metadata 后视频卡必须出现裁切渐变');
  assert.equal(variantCards.video.buttonAriaExpanded, 'false',
    '[social] metadata 后视频按钮必须暴露 aria-expanded=false');
  assert.equal(variantCards.video.buttonAriaControls, `social-text-${variantCards.video.id}`,
    '[social] metadata 后视频按钮必须用 aria-controls 指向正文');

  const assertCollapsedCard = (card, label, { image = false, video = false, empty = false } = {}) => {
    assert.ok(card, `[social] ${label} 落地样本缺失`);
    assert.ok(Math.abs(card.height - SOCIAL_COLLAPSED_ROW_HEIGHT) < 0.5,
      `[social] ${label} 折叠整行外高必须为 ${SOCIAL_COLLAPSED_ROW_HEIGHT}px，实际 ${card.height}`);
    assert.ok(Math.abs(card.cardHeight - SOCIAL_COLLAPSED_CARD_HEIGHT) < 0.5,
      `[social] ${label} 卡片外高必须为 ${SOCIAL_COLLAPSED_CARD_HEIGHT}px，实际 ${card.cardHeight}`);
    assert.ok(Math.abs(card.textHeight - SOCIAL_COLLAPSED_BODY_HEIGHT) < 0.5,
      `[social] ${label} 正文区必须为 ${SOCIAL_COLLAPSED_BODY_HEIGHT}px，实际 ${card.textHeight}`);
    assert.equal(card.cardStyleHeight, `${SOCIAL_COLLAPSED_CARD_HEIGHT}px`,
      `[social] ${label} 必须由固定内联外高锁定`);
    assert.equal(card.textStyleHeight, `${SOCIAL_COLLAPSED_BODY_HEIGHT}px`,
      `[social] ${label} 必须由固定内联正文高锁定`);
    assert.ok(Math.abs(card.slotHeight - SOCIAL_EXPAND_SLOT_HEIGHT) < 0.5,
      `[social] ${label} 按钮槽高必须为 ${SOCIAL_EXPAND_SLOT_HEIGHT}px，实际 ${card.slotHeight}`);
    assert.ok(card.textScrollWidth <= card.textClientWidth + 1
      && card.cardScrollWidth <= card.cardClientWidth + 1,
    `[social] ${label} 不得横向溢出`);
    if (image) {
      assert.ok(card.imageHeight > SOCIAL_COLLAPSED_BODY_HEIGHT,
        `[social] ${label} 的正文图片应先以自然高度存在，实际 ${card.imageHeight}`);
      assert.ok(card.visibleImageHeight <= SOCIAL_COLLAPSED_BODY_HEIGHT + 1,
        `[social] ${label} 的图片必须裁在正文区内，实际可见 ${card.visibleImageHeight}`);
      assert.equal(card.textMaxHeight, `${SOCIAL_COLLAPSED_BODY_HEIGHT}px`,
        `[social] ${label} 必须有 max-height 上限`);
      assert.equal(card.textOverflowY, 'hidden',
        `[social] ${label} 必须用 overflow:hidden 裁切`);
      assert.equal(card.hasExpandButton, true, `[social] ${label} 应提供展开按钮`);
      assert.equal(card.buttonAriaExpanded, 'false', `[social] ${label} 展开按钮应为折叠态`);
      assert.equal(card.buttonAriaControls, `social-text-${card.id}`,
        `[social] ${label} 的 aria-controls 必须指向正文`);
    }
    if (video) {
      assert.ok(card.videoRectHeight > SOCIAL_COLLAPSED_BODY_HEIGHT,
        `[social] ${label} 的视频应先以自然高度存在，实际 ${card.videoRectHeight}`);
      assert.ok(card.visibleVideoHeight <= SOCIAL_COLLAPSED_BODY_HEIGHT + 1,
        `[social] ${label} 的视频必须裁在正文区内，实际可见 ${card.visibleVideoHeight}`);
      assert.equal(card.textMaxHeight, `${SOCIAL_COLLAPSED_BODY_HEIGHT}px`,
        `[social] ${label} 必须有 max-height 上限`);
      assert.equal(card.textOverflowY, 'hidden',
        `[social] ${label} 必须用 overflow:hidden 裁切`);
      assert.equal(card.hasExpandButton, true, `[social] ${label} 应提供展开按钮`);
      assert.equal(card.buttonAriaExpanded, 'false', `[social] ${label} 展开按钮应为折叠态`);
      assert.equal(card.buttonAriaControls, `social-text-${card.id}`,
        `[social] ${label} 的 aria-controls 必须指向正文`);
    }
    if (empty) {
      assert.ok(String(card.placeholder).includes('暂无正文'),
        `[social] ${label} 空正文必须显示「暂无正文」`);
      assert.equal(card.hasExpandButton, false, `[social] ${label} 不应有展开按钮`);
    }
  };
  assertCollapsedCard(variantCards.bodyimage, 'bodyimage', { image: true });
  assert.equal(variantCards.bodyimage.imageSrcScheme, 'http',
    '[social] bodyimage 必须来自真实 HTTP 图片，不是 data URI');
  assert.equal(variantCards.bodyimage.imageSrcPath, '/delayed-image.svg',
    '[social] bodyimage 的图片路径必须指向延迟媒体服务');
  assert.equal(variantCards.bodyimage.imageComplete, true,
    '[social] bodyimage 的 HTTP 图片必须已完成解码');
  assert.ok(variantCards.bodyimage.imageNaturalWidth > 0
    && variantCards.bodyimage.imageNaturalHeight > 150,
  '[social] bodyimage 的 HTTP 图片必须有真实自然尺寸');
  assert.ok(variantCards.bodyimage.innerHeight > SOCIAL_COLLAPSED_BODY_HEIGHT,
    '[social] bodyimage 的自然高度内层必须超过折叠上限');
  assert.match(String(variantCards.bodyimage.textClass), /social-text-clipped/,
    '[social] 延迟图片加载后必须出现裁切渐变');
  assert.ok(cardsByKind.image.samples > 0
    && allCards.some((card) => card.kind === 'image' && card.imageSrcScheme === 'data'),
  '[social] 必须保留 data URI 图片作为补充场景');
  assertCollapsedCard(variantCards.short, 'short');
  assert.equal(variantCards.short.hasExpandButton, false, '[social] 短文不应有展开按钮');
  assertCollapsedCard(variantCards.empty, 'empty', { empty: true });
  assertCollapsedCard(variantCards.imagefail, 'imagefail');
  assert.equal(variantCards.imagefail.imageSrcScheme, 'http',
    '[social] 失败图片场景必须来自真实 HTTP 请求');
  assert.equal(variantCards.imagefail.imageComplete, true,
    '[social] 失败图片必须已完成错误收敛');
  assert.equal(variantCards.imagefail.imageNaturalWidth, 0,
    '[social] 失败图片的自然宽度应为 0');
  assert.equal(variantCards.imagefail.hasExpandButton, false,
    '[social] 短文 + 失败图片不应产生展开按钮');
  assert.doesNotMatch(String(variantCards.imagefail.textClass), /social-text-clipped/,
    '[social] 失败图片不应产生裁切渐变');
  assertCollapsedCard(variantCards.video, 'video', { video: true });
  assert.equal(variantCards.video.videoPosterScheme, 'http',
    '[social] 视频 poster 必须来自真实 HTTP 请求');
  assert.equal(variantCards.video.videoPosterPath, '/poster.svg',
    '[social] 视频 poster 路径必须指向延迟媒体服务');
  assert.equal(variantCards.video.videoSrcScheme, 'http',
    '[social] 视频源必须来自真实 HTTP 请求');
  assert.equal(variantCards.video.videoSrcPath, '/video.mp4',
    '[social] 视频源路径必须指向本地延迟媒体服务');
  assert.ok(variantCards.video.videoReadyState >= 1,
    '[social] 视频元数据必须已加载');
  assert.ok(variantCards.video.videoWidth > 0 && variantCards.video.videoHeight > 0,
    '[social] 视频必须暴露真实自然尺寸');
  assert.ok(videoMetadataEvents.length > 0,
    '[social] 必须记录到视频 loadedmetadata/loadeddata 事件');
  if (MUTATION !== 'no-social-media-remeasure') {
    assertCollapsedCard(variantCards.replacedLong, 'replaced-long');
    assert.ok(variantCards.replacedLong.innerHeight > SOCIAL_COLLAPSED_BODY_HEIGHT,
      '[social] body HTML 替换为长文后内层自然高度必须增长');
    assert.equal(variantCards.replacedLong.hasExpandButton, true,
      '[social] body HTML 替换为长文后必须重新出现展开按钮');
    assert.match(String(variantCards.replacedLong.textClass), /social-text-clipped/,
      '[social] body HTML 替换为长文后必须重新出现裁切渐变');
    assertCollapsedCard(variantCards.replacedShort, 'replaced-short');
    assert.equal(variantCards.replacedShort.hasExpandButton, false,
      '[social] body HTML 替换回短文后必须移除展开按钮');
    assert.doesNotMatch(String(variantCards.replacedShort.textClass), /social-text-clipped/,
      '[social] body HTML 替换回短文后必须移除裁切渐变');
  }

  assert.equal(actionSummary.length, MUTATION === 'no-social-media-remeasure' ? 0 : 3,
    '[social] 展开/收起动作样本必须覆盖 bodyimage、long 与 video');
  for (const action of actionSummary) {
    const label = `[social][${action.name}]`;
    assert.ok(action.frameCount > 10, `${label} 动作逐帧采样不足：${action.frameCount}`);
    assert.equal(action.overlapFrames, 0,
      `${label} 展开/收起往返不得重叠；max=${action.maxOverlapCount}`);
    assert.equal(action.horizontalOverflowFrames, 0, `${label} 动作帧不得横向溢出`);
    assert.equal(action.slotHeightViolationFrames, 0, `${label} 动作帧按钮槽高必须恒为 26px`);
    assert.ok(action.expandedFrames > 0, `${label} 动作帧必须真实记录到展开态`);
    assert.equal(action.before.buttonAriaExpanded, 'false', `${label} 起始应为折叠态`);
    assert.equal(action.before.buttonAriaControls, `social-text-${action.id}`,
      `${label} aria-controls 必须指向正文`);
    assert.equal(action.expanded.buttonAriaExpanded, 'true', `${label} 点击后应为展开态`);
    assert.equal(action.expanded.cardStyleHeight, '', `${label} 展开后必须退出固定外高`);
    assert.equal(action.expanded.textStyleHeight, '', `${label} 展开后必须退出固定正文高`);
    assert.ok(action.expanded.textHeight > SOCIAL_COLLAPSED_BODY_HEIGHT + 1,
      `${label} 展开后正文必须实际增长，实际 ${action.expanded.textHeight}`);
    assert.ok(action.expanded.height > action.before.height + 1,
      `${label} 展开后卡片必须实际增长`);
    assert.ok(action.expanded.cardHeight > action.before.cardHeight + 1,
      `${label} 展开后卡片内框必须实际增长`);
    assert.ok(action.scrollHeightExpanded > action.scrollHeightBefore + 1,
      `${label} 展开后滚动总高必须增长`);
    assert.equal(action.after.buttonAriaExpanded, 'false', `${label} 收起后应回到折叠态`);
    assert.ok(Math.abs(action.after.height - SOCIAL_COLLAPSED_ROW_HEIGHT) < 0.5,
      `${label} 收起后整行外高必须回到 ${SOCIAL_COLLAPSED_ROW_HEIGHT}px，实际 ${action.after.height}`);
    assert.ok(Math.abs(action.after.cardHeight - action.before.cardHeight) < 0.5,
      `${label} 收起后外高必须还原，before=${action.before.height}, after=${action.after.height}`);
    assert.ok(Math.abs(action.after.textHeight - SOCIAL_COLLAPSED_BODY_HEIGHT) < 0.5,
      `${label} 收起后正文区必须还原为 ${SOCIAL_COLLAPSED_BODY_HEIGHT}px`);
    assert.equal(action.after.cardStyleHeight, `${SOCIAL_COLLAPSED_CARD_HEIGHT}px`,
      `${label} 收起后必须恢复固定外高`);
    assert.equal(action.after.textStyleHeight, `${SOCIAL_COLLAPSED_BODY_HEIGHT}px`,
      `${label} 收起后必须恢复固定正文高`);
    assert.ok(Math.abs(action.after.slotHeight - SOCIAL_EXPAND_SLOT_HEIGHT) < 0.5,
      `${label} 收起后按钮槽高必须还原`);
    assert.ok(Math.abs(action.scrollTopExpanded - action.scrollTopBefore) <= 1,
      `${label} 展开期间滚动位置必须静止；before=${action.scrollTopBefore}, expanded=${action.scrollTopExpanded}`);
    assert.ok(Math.abs(action.scrollTopAfter - action.scrollTopBefore) <= 1,
      `${label} 收起后滚动位置必须还原；before=${action.scrollTopBefore}, after=${action.scrollTopAfter}`);
    assert.ok(Math.abs(action.scrollHeightAfter - action.scrollHeightBefore) < 1,
      `${label} 收起后滚动总高必须还原`);
    assert.equal(action.lastFrame?.buttonAriaExpanded, 'false',
      `${label} 最后一帧必须已回到折叠态`);
    assert.ok(Math.abs(action.lastFrame?.height - SOCIAL_COLLAPSED_ROW_HEIGHT) < 0.5,
      `${label} 最后一帧整行外高必须回到 ${SOCIAL_COLLAPSED_ROW_HEIGHT}px`);
    if (action.name === 'bodyimage') {
      assert.ok(action.before.visibleImageHeight <= SOCIAL_COLLAPSED_BODY_HEIGHT + 1,
        `${label} 折叠时图片必须裁在正文区内，实际 ${action.before.visibleImageHeight}`);
      assert.ok(action.expanded.visibleImageHeight > SOCIAL_COLLAPSED_BODY_HEIGHT + 1,
        `${label} 展开后图片必须可见，实际 ${action.expanded.visibleImageHeight}`);
    }
    if (action.name === 'video') {
      assert.equal(action.expanded.videoNodeId, action.before.videoNodeId,
        `${label} 展开期间视频节点不得重建`);
      assert.equal(action.after.videoNodeId, action.before.videoNodeId,
        `${label} 收起后视频节点不得重建`);
      assert.ok(action.before.visibleVideoHeight <= SOCIAL_COLLAPSED_BODY_HEIGHT + 1,
        `${label} 折叠时视频必须裁在正文区内，实际 ${action.before.visibleVideoHeight}`);
      assert.ok(action.expanded.visibleVideoHeight > SOCIAL_COLLAPSED_BODY_HEIGHT + 1,
        `${label} 展开后视频必须可见，实际 ${action.expanded.visibleVideoHeight}`);
    }
  }
}

async function main() {
  await fsp.mkdir(EVIDENCE_DIR, { recursive: true });
  for (const name of await fsp.readdir(EVIDENCE_DIR)) {
    if (name === 'metrics.json' || name === 'failure.png'
      || name.endsWith('-failure.png')
      || (name.endsWith('.png') && (name.includes('layout-') || name.includes('social-')))) {
      await fsp.unlink(path.join(EVIDENCE_DIR, name));
    }
  }
  /* 浏览器 profile 必须放在仓库外：Vite watcher 会跟随 tmp/ 下的锁文件并因
     Windows EBUSY 退出。证据截图/JSON 仍落仓库 tmp/ui-layout-evidence。 */
  const profileDirs = [];
  const makeProfileDir = async () => {
    const dir = await fsp.mkdtemp(path.join(os.tmpdir(), 'fluxreader-ui-layout-'));
    const resolvedDir = path.resolve(dir);
    const resolvedTemp = path.resolve(os.tmpdir());
    if (!resolvedDir.startsWith(`${resolvedTemp}${path.sep}`)) {
      throw new Error(`拒绝在系统临时目录外创建浏览器 profile：${resolvedDir}`);
    }
    profileDirs.push(dir);
    return dir;
  };
  let profileDir = await makeProfileDir();

  let vite;
  let media;
  let browser;
  let cdp;
  const evidence = {
    generatedAt: new Date().toISOString(),
    mutation: MUTATION,
    g2TraceConfig: G2_TRACE ? {
      runtime: 'tools/ui-layout/g2-trace-runtime.js',
      timelineInjection: 'tools/ui-layout/g2-timeline-inject.mjs',
      traceFile: 'g2-trace.json',
    } : null,
    viewports: {},
    keyboardViewports: {},
    browser: null,
    consoleMessages: [],
    consoleErrors: [],
    networkFailures: [],
    exceptions: [],
    startup: {
      timeoutMs: STARTUP_TIMEOUT_MS,
      attempts: [],
      retried: false,
      faultInjection: FAULT_MODE === 'none' ? null : {
        mode: FAULT_MODE,
        delayMs: FAULT_DELAY_MS,
        armedAttempt: 1,
      },
    },
  };
  let activeViewportLabel = null;

  try {
    media = await startMediaServer();
    evidence.media = { url: media.url };
    let startupReady = false;
    for (let attempt = 1; attempt <= 2; attempt++) {
      const attemptStartedAt = performance.now();
      const attemptEvidence = {
        attempt,
        startedAtMs: 0,
        status: 'starting',
        stages: {},
      };
      evidence.startup.attempts.push(attemptEvidence);
      let faultActive = false;
      try {
        const viteStartedAt = performance.now();
        vite = await startVite();
        const viteReadyAt = performance.now();
        attemptEvidence.stages.vite = {
          startedAtMs: Math.round(viteStartedAt - attemptStartedAt),
          readyAtMs: Math.round(viteReadyAt - attemptStartedAt),
          durationMs: Math.round(viteReadyAt - viteStartedAt),
        };

        const browserStartedAt = performance.now();
        browser = await startBrowser(profileDir);
        const browserReadyAt = performance.now();
        attemptEvidence.stages.browser = {
          startedAtMs: Math.round(browserStartedAt - attemptStartedAt),
          readyAtMs: Math.round(browserReadyAt - attemptStartedAt),
          durationMs: Math.round(browserReadyAt - browserStartedAt),
        };
        evidence.browser = { path: browser.browserPath, headless: true };
        const page = await connectPage(browser.debugPort, 'about:blank');
        cdp = page.cdp;
        cdp.on('Runtime.consoleAPICalled', (event) => {
          const text = event.args.map((arg) => arg.value ?? arg.description ?? '').join(' ');
          evidence.consoleMessages.push({ type: event.type, text });
          if (event.type === 'error') evidence.consoleErrors.push(text);
        });
        cdp.on('Runtime.exceptionThrown', (event) => {
          evidence.exceptions.push(event.exceptionDetails.exception?.description ?? event.exceptionDetails.text);
        });
        cdp.on('Network.loadingFailed', (event) => {
          evidence.networkFailures.push({
            errorText: event.errorText,
            blockedReason: event.blockedReason ?? null,
            type: event.type,
          });
        });

        if (G2_TRACE) {
          await cdp.send('Page.addScriptToEvaluateOnNewDocument', { source: G2_TRACE_SOURCE });
          await cdp.send('Fetch.enable', {
            patterns: [{ urlPattern: '*Timeline.tsx*', requestStage: 'Response' }],
          });
          cdp.on('Fetch.requestPaused', async (params) => {
            try {
              if (!/\/src\/components\/Timeline\.tsx/.test(params.request.url) || params.responseStatusCode == null) {
                await cdp.send('Fetch.continueRequest', { requestId: params.requestId });
                return;
              }
              const body = await cdp.send('Fetch.getResponseBody', { requestId: params.requestId });
              const source = body.base64Encoded ? Buffer.from(body.body, 'base64').toString('utf8') : body.body;
              const injected = injectTimelineTrace(source);
              await cdp.send('Fetch.fulfillRequest', {
                requestId: params.requestId,
                responseCode: params.responseStatusCode,
                responseHeaders: params.responseHeaders ?? [],
                body: Buffer.from(injected, 'utf8').toString('base64'),
              });
            } catch (error) {
              evidence.exceptions.push(`g2 Timeline 模块注入失败：${error?.stack ?? error}`);
              try {
                await cdp.send('Fetch.continueResponse', { requestId: params.requestId });
              } catch {
                try {
                  await cdp.send('Fetch.continueRequest', { requestId: params.requestId });
                } catch {
                  /* 页面可能已销毁 */
                }
              }
            }
          });
        }

        await setViewport(cdp, 1440, 900);
        const navigateStartedAt = performance.now();
        await navigate(cdp, vite.url);
        faultActive = FAULT_MODE === 'bootstrap-error' && attempt === 1;
        if (faultActive) {
          await installStartupFault(cdp, FAULT_DELAY_MS);
          attemptEvidence.faultInjection = { mode: FAULT_MODE, delayMs: FAULT_DELAY_MS };
        }
        const pageStartedAt = performance.now();
        const pageDeadline = pageStartedAt + STARTUP_TIMEOUT_MS;
        const firstMount = await waitForStartupPhase(
          cdp,
          `document.querySelector('.app-root') !== null`,
          '页面首挂载',
          pageDeadline - performance.now(),
        );
        const splashGone = await waitForStartupPhase(
          cdp,
          faultActive
            ? `document.querySelector('.app-root') !== null
              && document.querySelector('.app-loading-splash') === null
              && window.__uiLayoutFault?.allowSplash === true`
            : `document.querySelector('.app-root') !== null
              && document.querySelector('.app-loading-splash') === null`,
          'splash 消失',
          pageDeadline - performance.now(),
        );
        attemptEvidence.stages.page = {
          navigateStartedAtMs: Math.round(navigateStartedAt - attemptStartedAt),
          firstMountStartedAtMs: Math.round(firstMount.startedAt - attemptStartedAt),
          firstMountAtMs: Math.round(firstMount.endedAt - attemptStartedAt),
          firstMountMs: Math.round(firstMount.elapsedMs),
          splashWaitStartedAtMs: Math.round(splashGone.startedAt - attemptStartedAt),
          splashGoneAtMs: Math.round(splashGone.endedAt - attemptStartedAt),
          splashWaitMs: Math.round(splashGone.elapsedMs),
        };
        attemptEvidence.status = 'ok';
        attemptEvidence.durationMs = Math.round(performance.now() - attemptStartedAt);
        evidence.startup.activeAttempt = attempt;
        console.log(
          `[ui-layout] startup attempt=${attempt}`
          + ` vite=${attemptEvidence.stages.vite.durationMs}ms`
          + ` browser=${attemptEvidence.stages.browser.durationMs}ms`
          + ` firstMount=${Math.round(firstMount.elapsedMs)}ms`
          + ` splash=${Math.round(splashGone.elapsedMs)}ms`,
        );
        startupReady = true;
        break;
      } catch (error) {
        attemptEvidence.status = 'failed';
        attemptEvidence.durationMs = Math.round(performance.now() - attemptStartedAt);
        attemptEvidence.failure = error?.stack ?? String(error);
        if (error instanceof StartupTimeoutError) {
          attemptEvidence.phase = error.phase ?? null;
          attemptEvidence.bootstrapError = error.bootstrapError ?? null;
          attemptEvidence.pageState = error.pageState ?? null;
          attemptEvidence.phaseElapsedMs = error.elapsedMs ?? null;
          attemptEvidence.detectedEarly = Boolean(error.detectedEarly);
          if (faultActive) {
            attemptEvidence.faultInjection = {
              ...attemptEvidence.faultInjection,
              injectedAfterMs: error.pageState?.fault?.elapsedMs ?? null,
            };
          }
        }
        if (error instanceof StartupTimeoutError && attempt === 1) {
          evidence.startup.retried = true;
          cdp?.close();
          cdp = null;
          terminateTree(browser?.child);
          browser = null;
          terminateTree(vite?.child);
          vite = null;
          evidence.consoleMessages.length = 0;
          evidence.consoleErrors.length = 0;
          evidence.networkFailures.length = 0;
          evidence.exceptions.length = 0;
          profileDir = await makeProfileDir();
          await delay(500);
          continue;
        }
        throw error;
      }
    }
    if (!startupReady) throw new Error('ui-layout 启动重试后仍未就绪');
    if (FAULT_MODE !== 'none') {
      const attempts = evidence.startup.attempts;
      assert.equal(attempts.length, 2,
        `[fault] 启动故障应只触发一次重试，实际 attempts=${attempts.length}`);
      const first = attempts[0];
      const second = attempts[1];
      assert.equal(first.status, 'failed', '[fault] 第一次启动应失败');
      assert.equal(first.detectedEarly, true,
        '[fault] .bootstrap-error 必须在采样周期内被提前发现');
      assert.match(String(first.bootstrapError), /故障注入/,
        `[fault] 第一次启动错误文本缺失：${first.bootstrapError}`);
      assert.ok(first.phaseElapsedMs != null && first.phaseElapsedMs < STARTUP_TIMEOUT_MS,
        `[fault] 启动错误不得等满预算：phaseElapsedMs=${first.phaseElapsedMs}`);
      assert.ok(first.durationMs != null && first.durationMs < 45000,
        `[fault] 启动错误必须在约 25s 后快速失败：durationMs=${first.durationMs}`);
      assert.ok(first.faultInjection?.injectedAfterMs >= 24000
        && first.faultInjection?.injectedAfterMs < 27000,
      `[fault] 注入时间应约为 25s：injectedAfterMs=${first.faultInjection?.injectedAfterMs}`);
      assert.equal(second.status, 'ok', '[fault] 单次重试后的启动应成功');
      assert.equal(evidence.startup.retried, true, '[fault] 应记录启动重试');
      evidence.mediaRequests = media.requests;
      await saveJson(path.join(EVIDENCE_DIR, 'metrics.json'), evidence);
      console.log(JSON.stringify({
        ok: true,
        fault: {
          mode: FAULT_MODE,
          delayMs: FAULT_DELAY_MS,
          firstAttempt: {
            status: first.status,
            phase: first.phase,
            detectedEarly: first.detectedEarly,
            phaseElapsedMs: first.phaseElapsedMs,
            durationMs: first.durationMs,
            bootstrapError: first.bootstrapError,
            injectedAfterMs: first.faultInjection?.injectedAfterMs,
          },
          secondAttempt: { status: second.status, durationMs: second.durationMs },
        },
        evidence: `${EVIDENCE_REL}/metrics.json`,
        startup: evidence.startup,
      }, null, 2));
      return;
    }
    await delay(300);
    await applyRuntimeMutation(cdp);

    for (const viewport of SELECTED_VIEWPORTS) {
      activeViewportLabel = viewport.label;
      await setViewport(cdp, viewport.width, viewport.height);
      await delay(220);
      evidence.viewports[viewport.label] = {
        viewport: { width: viewport.width, height: viewport.height },
        layouts: {},
        socialHydration: null,
        keyboardA11y: null,
      };
      if (CHECK_GROUP === 'all') await runLayoutChecks(cdp, evidence.viewports[viewport.label], viewport.label);
      await runGalleryLoadingCheck(cdp, evidence.viewports[viewport.label], viewport.label, media);
      await runGalleryWindowCheck(cdp, evidence.viewports[viewport.label], viewport.label);
    }
    for (const viewport of CHECK_GROUP === 'all' ? SELECTED_VIEWPORTS : []) {
      activeViewportLabel = viewport.label;
      await setViewport(cdp, viewport.width, viewport.height);
      await delay(220);
      await runSocialCheck(cdp, evidence.viewports[viewport.label], viewport.label, SOCIAL_ID_BASE[viewport.label], media);
    }
    for (const viewport of CHECK_GROUP === 'all' ? SELECTED_KEYBOARD_VIEWPORTS : []) {
      activeViewportLabel = viewport.label;
      await setViewport(cdp, viewport.width, viewport.height);
      await delay(220);
      evidence.keyboardViewports[viewport.label] = {
        viewport: { width: viewport.width, height: viewport.height },
        keyboardA11y: null,
      };
      await runKeyboardA11yChecks(cdp, evidence.keyboardViewports[viewport.label], viewport.label);
    }

    evidence.mediaRequests = media.requests;
    assert.deepEqual(evidence.consoleErrors, [], `浏览器 console.error：${JSON.stringify(evidence.consoleErrors)}`);
    assert.deepEqual(evidence.exceptions, [], `浏览器未捕获异常：${JSON.stringify(evidence.exceptions)}`);
    await saveJson(path.join(EVIDENCE_DIR, 'metrics.json'), evidence);
    console.log(JSON.stringify({
      ok: true,
      evidence: `${EVIDENCE_REL}/metrics.json`,
      mutation: MUTATION,
      startup: evidence.startup,
      viewports: Object.fromEntries(Object.entries(evidence.viewports).map(([label, value]) => [label, {
        layouts: Object.fromEntries(Object.entries(value.layouts).map(([layout, run]) => [layout, {
          inlineHeight: run.top.wrapInlineHeight,
          usedHeight: run.top.wrapUsedHeight,
          footerClearOfScrollerAtTop: run.top.footerClearOfScroller,
          footerVisibleAtBottom: run.bottom.footerInScrollerViewport,
          footerWithinScrollerBottom: run.bottom.footerWithinScrollerBottom,
        }])),
        galleryLoading: value.galleryLoading ? {
          phases: value.galleryLoading.phases.map((phase) => ({
            anchorId: phase.before.anchor.id,
            beforeTop: phase.before.anchor.top,
            afterTop: phase.after.anchor?.top,
            maxAnchorDrift: phase.maxAnchorDrift,
            settledAnchorDrift: phase.settledAnchorDrift,
          })),
          footerVisibleAtBottom: value.galleryLoading.bottom.footerInScrollerViewport,
          footerWithinScrollerBottom: value.galleryLoading.bottom.footerWithinScrollerBottom,
        } : null,
        socialHydration: value.socialHydration ? {
          cardsByKind: value.socialHydration.cardsByKind,
          imageFrames: value.socialHydration.imageFrames,
          imageBoundaryViolations: value.socialHydration.imageBoundaryViolations,
          longUncollapsedFrames: value.socialHydration.longUncollapsedFrames,
          overlapFrames: value.socialHydration.overlapFrames,
          scrollHeightSpread: value.socialHydration.scrollHeightSpread,
          cumulativeLayoutShift: value.socialHydration.cumulativeLayoutShift,
          cumulativeLayoutShiftOutsideSocialBody: value.socialHydration.cumulativeLayoutShiftOutsideSocialBody,
          videoInitial: value.socialHydration.videoInitial,
          videoMetadata: value.socialHydration.videoMetadata,
          videoPoster: value.socialHydration.videoPoster,
        } : null,
      }])),
      keyboardViewports: Object.fromEntries(Object.entries(evidence.keyboardViewports).map(([label, value]) => [label, {
        responsiveGeometry: value.keyboardA11y.responsiveGeometry,
        settingsGeometry: value.keyboardA11y.settingsGeometry,
        settingsAxControlCount: value.keyboardA11y.settingsAxControlCount,
        keyboard: value.keyboardA11y.keyboard,
        focusMigration: value.keyboardA11y.focusMigration,
        sidebarPaddingClicks: value.keyboardA11y.sidebarPaddingClicks,
        sidebarSpace: value.keyboardA11y.sidebarSpace,
      }])),
    }, null, 2));
  } catch (error) {
    evidence.mediaRequests = media?.requests ?? [];
    evidence.failure = error?.stack ?? String(error);
    evidence.vite = { exited: vite?.child.exitCode ?? null, output: vite?.getOutput?.() ?? '' };
    try {
      if (cdp) {
        evidence.pageState = await evalValue(cdp, `(() => ({
          readyState: document.readyState,
          title: document.title,
          url: location.href,
          bodyText: document.body.innerText.slice(0, 1000),
          root: !!document.querySelector('.app-root'),
          splash: !!document.querySelector('.app-loading-splash'),
          resources: performance.getEntriesByType('resource').slice(-30).map((entry) => ({
            name: entry.name,
            duration: entry.duration,
            transferSize: entry.transferSize,
          })),
        }))()`);
      }
    } catch { /* 保留原始失败 */ }
    try {
      if (cdp) await screenshot(cdp, path.join(EVIDENCE_DIR, `${activeViewportLabel ?? 'unknown'}-failure.png`));
    } catch { /* 保留原始失败 */ }
    await saveJson(path.join(EVIDENCE_DIR, 'metrics.json'), evidence);
    throw error;
  } finally {
    cdp?.close();
    terminateTree(browser?.child);
    terminateTree(vite?.child);
    await media?.close?.();
    await delay(300);
    for (const dir of profileDirs) {
      await fsp.rm(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    }
  }
}

main().catch((error) => {
  console.error(error?.stack ?? error);
  process.exitCode = 1;
});
