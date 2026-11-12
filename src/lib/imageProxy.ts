// Note: 图片编码bytes预算与使用中URL租约 — 见 .agents/notes/implemented/feature/2026-10-07-封面代理与失败占位.md
import { api, extractErrorCode } from './api';

const PROXY_HOSTS = new Set(['cdnfile.sspai.com', 'rssfile.sspai.com']);
const PROXY_HOST_SUFFIXES = ['.doubanio.com'];
export function needsImageProxy(src: string): boolean {
  try { const host = new URL(src).hostname.toLowerCase(); return PROXY_HOSTS.has(host) || PROXY_HOST_SUFFIXES.some(s => host === s.slice(1) || host.endsWith(s)); }
  catch { return false; }
}
export function canProxyImages(): boolean { return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window; }
const MiB = 1024 * 1024;
export const IMAGE_LIMITS = Object.freeze({ active: 4, pending: 32, payload: 128 * MiB, image: 25 * MiB,
  idleBytes: 64 * MiB, idleEntries: 256, leases: 256, failures: 1024, transientTtl: 30_000, permanentTtl: 600_000 });
export type ImageFailure = 'network' | 'permanent' | 'not-image' | 'empty' | 'decode' | 'too-large' | 'unavailable';
export type ImageSnapshot =
  | { status: 'queued' | 'loading' | 'deferred' }
  | { status: 'ready'; src: string; direct: boolean }
  | { status: 'failed'; reason: ImageFailure };
const DEFERRED: ImageSnapshot = { status: 'deferred' };
const QUEUED: ImageSnapshot = { status: 'queued' };
const LOADING: ImageSnapshot = { status: 'loading' };
export interface ImageLease {
  readonly admitted: boolean;
  readonly key: string;
  readonly epoch: number;
  readonly url: string;
  readonly pageUrl?: string;
  snapshot(): ImageSnapshot;
  subscribe(listener: () => void): () => void;
  release(): void;
  failDecode(): void;
}
interface Entry {
  key: string; epoch: number; url: string; pageUrl?: string; priority: number;
  refs: number; listeners: Set<() => void>; value: ImageSnapshot; touched: number;
  bytes: number; blob?: string; requestId?: string; reserved: boolean; cancelled: boolean;
  started: boolean; cancelSent: boolean; failureUntil: number; retryAt: number;
}
const entries = new Map<string, Entry>();
// Retired entries are still owned by leases/IPC until DOM cleanup and completion.
const owned = new Set<Entry>();
let epoch = 0;
let suspended = false;
let leaseCount = 0;
let serial = 0;
let pumping = false;
let capacityListener: (() => void) | undefined;
let capacityNoticeQueued = false;
export function bindImageCapacityListener(listener: () => void): void { capacityListener = listener; }
function capacityAvailable(): void {
  if (capacityNoticeQueued) return;
  capacityNoticeQueued = true;
  queueMicrotask(() => { capacityNoticeQueued = false; if (!suspended && leaseCount < IMAGE_LIMITS.leases) capacityListener?.(); });
}
let timer: ReturnType<typeof setTimeout> | undefined;
const requestPrefix = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
export function imageEpoch(): number { return epoch; }
export function imagesSuspended(): boolean { return suspended; }

export function imagePageReferer(url: string, pageUrl?: string): string | undefined {
  if (!pageUrl || !/^https?:\/\//.test(pageUrl)) return undefined;
  try { new URL(pageUrl); if (pageUrl === `${new URL(url).origin}/`) return undefined; return pageUrl; }
  catch { return undefined; }
}
export function imageResourceKey(url: string, pageUrl?: string): string {
  return JSON.stringify([epoch, url, needsImageProxy(url) && canProxyImages() ? imagePageReferer(url, pageUrl) ?? null : null]);
}
function publish(entry: Entry, value: ImageSnapshot): void {
  entry.value = value;
  for (const listener of [...entry.listeners]) listener();
}
function forget(entry: Entry): void {
  if (entry.refs || entry.reserved) return;
  if (entry.blob) { URL.revokeObjectURL(entry.blob); entry.blob = undefined; entry.bytes = 0; }
  if (entries.get(entry.key) === entry) entries.delete(entry.key);
  owned.delete(entry);
}
function trim(): void {
  const now = Date.now();
  const idle = [...owned].filter(e => !e.refs && !e.reserved).sort((a, b) => a.touched - b.touched);
  let idleBytes = idle.reduce((n, e) => n + e.bytes, 0);
  let idleReady = idle.filter(e => !!e.blob).length;
  let failures = [...owned].filter(e => e.value.status === 'failed').length;
  for (const entry of idle) {
    if (entry.epoch !== epoch || entries.get(entry.key) !== entry || entry.cancelled
      || (entry.value.status === 'failed' && (entry.failureUntil <= now || failures > IMAGE_LIMITS.failures))
      || (entry.blob && (idleBytes > IMAGE_LIMITS.idleBytes || idleReady > IMAGE_LIMITS.idleEntries))
      || (!entry.blob && entry.value.status !== 'failed')) {
      if (entry.blob) { idleBytes -= entry.bytes; --idleReady; }
      if (entry.value.status === 'failed') --failures;
      forget(entry);
    }
  }
}
function usedBytes(): number { let n = 0; for (const e of owned) n += e.bytes + (e.reserved ? IMAGE_LIMITS.image : 0); return n; }
function reserve(): boolean {
  if (usedBytes() + IMAGE_LIMITS.image <= IMAGE_LIMITS.payload) return true;
  for (const entry of [...owned].filter(e => e.refs === 0 && !e.reserved && e.blob).sort((a, b) => a.touched - b.touched)) {
    forget(entry);
    if (usedBytes() + IMAGE_LIMITS.image <= IMAGE_LIMITS.payload) return true;
  }
  return false;
}
function cancel(entry: Entry): void {
  entry.cancelled = true;
  if (entry.started && entry.requestId && !entry.cancelSent) {
    entry.cancelSent = true;
    void api.cancelImage(entry.requestId).catch(error => console.warn('[image] cancel failed', error));
  }
}
function wakeLater(at: number): void {
  if (timer !== undefined) return;
  timer = setTimeout(() => { timer = undefined; pump(); }, Math.max(1, Math.min(1000, at - Date.now())));
}
function fail(entry: Entry, reason: ImageFailure): void {
  entry.failureUntil = Date.now() + (reason === 'permanent' ? IMAGE_LIMITS.permanentTtl : IMAGE_LIMITS.transientTtl);
  entry.value = { status: 'failed', reason };
  trim();
  publish(entry, entry.value);
}
function pump(): void {
  if (pumping || suspended) return;
  pumping = true;
  try {
    trim();
    let active = [...owned].filter(e => e.reserved).length;
    let queued = [...owned].filter(e => !e.cancelled && e.value.status === 'queued').length;
    const candidates = [...owned].filter(e => e.refs && !e.cancelled && e.epoch === epoch && !e.reserved
      && (e.value.status === 'deferred' || e.value.status === 'queued'))
      .sort((a, b) => a.priority - b.priority || a.touched - b.touched);
    for (const entry of candidates) {
      if (entry.retryAt > Date.now()) { wakeLater(entry.retryAt); continue; }
      if (active < IMAGE_LIMITS.active && reserve()) {
        if (entry.value.status === 'queued') --queued;
        ++active;
        start(entry);
      } else if (entry.value.status === 'deferred' && queued < IMAGE_LIMITS.pending) {
        ++queued; publish(entry, QUEUED);
      }
    }
  } finally { pumping = false; }
}
function start(entry: Entry): void {
  entry.started = false; entry.cancelSent = false;
  entry.reserved = true;
  entry.requestId = `${requestPrefix}:${++serial}`;
  publish(entry, LOADING);
  void api.fetchImage(entry.url, entry.pageUrl, entry.requestId, () => {
    entry.started = true;
    if (entry.cancelled) cancel(entry);
  }, () => !entry.cancelled && !suspended && entry.epoch === epoch).then(bytes => {
    if (entry.cancelled || suspended || entry.epoch !== epoch || !entry.refs) return;
    if (!bytes) { fail(entry, 'unavailable'); return; }
    if (!bytes.length) { fail(entry, 'empty'); return; }
    const mime = sniffImageMime(bytes);
    if (!mime) { fail(entry, 'not-image'); return; }
    // reservation covers the encoded body until Blob creation completes; no base64/HTML copies.
    const blob = new Blob([bytes], { type: mime });
    entry.blob = URL.createObjectURL(blob);
    entry.bytes = bytes.byteLength;
    entry.reserved = false;
    publish(entry, { status: 'ready', src: entry.blob, direct: false });
  }).catch(error => {
    if (entry.cancelled || suspended || entry.epoch !== epoch || !entry.refs) return;
    const code = extractErrorCode(error);
    if (code === 'imageBusy') { entry.retryAt = Date.now() + 100; publish(entry, DEFERRED); }
    else if (code === 'imageCancelled') { entry.retryAt = Date.now() + 100; publish(entry, DEFERRED); }
    else fail(entry, code === 'imageGone' ? 'permanent' : code === 'responseTooLarge' ? 'too-large' : 'network');
  }).finally(() => {
    entry.reserved = false;
    entry.requestId = undefined;
    if (entry.cancelled || entry.epoch !== epoch) forget(entry);
    trim(); pump();
  });
}
export function acquireImage(url: string, pageUrl?: string, priority = 1): ImageLease {
  const key = imageResourceKey(url, pageUrl);
  const declined = (): ImageLease => ({
    admitted: false, key, epoch, url, pageUrl, snapshot: () => DEFERRED, subscribe: () => () => {}, release() {}, failDecode() {},
  });
  if (suspended) return declined();
  // Direct images own only local consumer state: no shared entry, quota or failure cache.
  if (!canProxyImages() || !needsImageProxy(url) || !/^https?:\/\//.test(url)) {
    const directEpoch = epoch;
    let value: ImageSnapshot = { status: 'ready', src: url, direct: true };
    let released = false;
    const listeners = new Set<() => void>();
    return {
      admitted: true, key, epoch: directEpoch, url, pageUrl,
      snapshot: () => value,
      subscribe(listener) { if (released) return () => {}; listeners.add(listener); return () => { listeners.delete(listener); }; },
      release() { released = true; listeners.clear(); },
      failDecode() { if (!released && directEpoch === epoch && !suspended) {
        value = { status: 'failed', reason: 'decode' };
        for (const listener of listeners) listener();
      } },
    };
  }
  trim();
  if (leaseCount >= IMAGE_LIMITS.leases) return declined();
  let entry = entries.get(key);
  if (entry && (entry.cancelled || (entry.value.status === 'failed' && entry.failureUntil <= Date.now()))) {
    entries.delete(key); forget(entry); entry = undefined;
  }
  if (!entry) {
    entry = { key, epoch, url, pageUrl: imagePageReferer(url, pageUrl), priority, refs: 0, listeners: new Set(),
      value: DEFERRED, touched: Date.now(), bytes: 0,
      reserved: false, cancelled: false, started: false, cancelSent: false, failureUntil: 0, retryAt: 0 };
    entries.set(key, entry); owned.add(entry);
  }
  const ownedEntry = entry;
  ownedEntry.refs++; leaseCount++; ownedEntry.priority = Math.min(priority, ownedEntry.priority); ownedEntry.touched = Date.now();
  let released = false;
  let localFailure: ImageSnapshot | null = null;
  const subscriptions = new Set<() => void>();
  const lease: ImageLease = {
    admitted: true, key, epoch, url, pageUrl: ownedEntry.pageUrl,
    snapshot: () => localFailure ?? ownedEntry.value,
    subscribe(listener) { if (released) return () => {}; subscriptions.add(listener); ownedEntry.listeners.add(listener);
      return () => { subscriptions.delete(listener); ownedEntry.listeners.delete(listener); }; },
    release() {
      if (released) return;
      released = true;
      for (const listener of subscriptions) ownedEntry.listeners.delete(listener);
      const wasFull = leaseCount === IMAGE_LIMITS.leases;
      subscriptions.clear(); ownedEntry.refs--; leaseCount--; ownedEntry.touched = Date.now();
      if (wasFull) capacityAvailable();
      if (!ownedEntry.refs && ['loading', 'queued', 'deferred'].includes(ownedEntry.value.status)) cancel(ownedEntry);
      trim(); pump();
    },
    // Mark locally; the DOM owner releases only after removing this Blob's src.
    failDecode() { if (!released && ownedEntry.epoch === epoch && !suspended) {
      localFailure = { status: 'failed', reason: 'decode' };
      for (const listener of subscriptions) listener();
    } },
  };
  pump();
  return lease;
}
/** Explicit retry invalidates a failed key, never revokes another displayed lease. */
export function retryImage(url: string, pageUrl?: string): void {
  const entry = entries.get(imageResourceKey(url, pageUrl));
  if (entry && (entry.value.status === 'failed' || entry.value.status === 'ready')) { entries.delete(entry.key); forget(entry); }
  pump();
}
export function suspendImages(): void {
  suspended = true;
  if (timer !== undefined) { clearTimeout(timer); timer = undefined; }
  for (const entry of owned) if (entry.reserved || entry.value.status === 'queued' || entry.value.status === 'deferred') {
    cancel(entry); publish(entry, DEFERRED);
  }
}
export function resumeImages(): void {
  suspended = false;
  // Consumers re-acquire at the restore busy boundary; cancelled leases never revive.
  pump();
}
export function resetImagesForLibraryRestore(): void {
  suspendImages(); ++epoch; entries.clear(); trim();
}
/** Counters describe encoded resources owned here, not JS/native/GPU heap. */
export function imageResourceStats() {
  const all = [...owned];
  return { epoch, leases: leaseCount, active: all.filter(e => e.reserved).length,
    pending: all.filter(e => !e.cancelled && e.value.status === 'queued').length,
    payloadBytes: usedBytes(), blobBytes: all.reduce((n, e) => n + e.bytes, 0),
    idleBytes: all.filter(e => !e.refs).reduce((n, e) => n + e.bytes, 0),
    ready: all.filter(e => e.blob).length, failures: all.filter(e => e.value.status === 'failed').length };
}
export function resetImagesForTest(): void {
  if (leaseCount) throw new Error('image leases still mounted');
  resetImagesForLibraryRestore(); resumeImages();
}
function sniffImageMime(b: Uint8Array): string | null {
  if (b.length >= 4 && b[0] === 137 && b[1] === 80 && b[2] === 78 && b[3] === 71) return 'image/png';
  if (b.length >= 3 && b[0] === 255 && b[1] === 216 && b[2] === 255) return 'image/jpeg';
  if (b.length >= 3 && b[0] === 71 && b[1] === 73 && b[2] === 70) return 'image/gif';
  if (b.length >= 12 && String.fromCharCode(...b.subarray(0, 4)) === 'RIFF' && String.fromCharCode(...b.subarray(8, 12)) === 'WEBP') return 'image/webp';
  if (b.length >= 12 && String.fromCharCode(...b.subarray(4, 8)) === 'ftyp') {
    const brand = String.fromCharCode(...b.subarray(8, 12));
    if (['avif', 'avis'].includes(brand)) return 'image/avif';
    if (['heic', 'heix', 'mif1'].includes(brand)) return 'image/heic';
  }
  if (b[0] === 66 && b[1] === 77) return 'image/bmp';
  if (b.length >= 4 && b[0] === 0 && b[1] === 0 && b[2] === 1 && b[3] === 0) return 'image/x-icon';
  const head = new TextDecoder().decode(b.subarray(0, 512)).trimStart().toLowerCase();
  return head.startsWith('<svg') || (head.startsWith('<?xml') && head.includes('<svg')) ? 'image/svg+xml' : null;
}





