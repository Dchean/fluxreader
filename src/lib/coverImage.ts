// Note: 封面状态机与失败原因分类（同一 URL 同会话不重复请求） — 见 .agents/notes/implemented/feature/2026-10-07-封面代理与失败占位.md
// 封面图片位的共享状态机（TASK-092 / REQ-106 ③）。
//
// 五处图片位（文章卡、播客卡、迷你播放条、全屏播放器、灯箱）共用这一份：
// - 取图路径：是否走代理只看 lib/imageProxy.ts 的 needsImageProxy（本文件不另写域名规则）；
//   需要代理 → api.fetchImage 拿字节（fetchProxiedImage 校验确为图片）转 data: URL；
//   不需要 → 直连原图（referrerPolicy=no-referrer，与修前一致），失败由 <img onError> 报告。
// - 失败回退：模块级缓存按 URL 记住失败，同一会话内同一 URL 不再发起请求；
//   组件据此渲染占位而不是破图。
// - 失效上报：失败时调用 report_broken_cover(article_id, url)；同一条目同一 URL 只上报一次，
//   上报本身失败只 console.warn，不冒泡。播放条/灯箱与卡片展示同一 cover 时读的是同一份
//   缓存、同一个去重集合，所以不会重复取图、也不会重复上报。
//
// 纯逻辑放在这里（无 React），组件 components/CoverImage.tsx 只做订阅与渲染，
// 前端回归网直接驱动本模块的函数。
import { api } from './api';
import { canProxyImages, fetchProxiedImage, needsImageProxy } from './imageProxy';

/** 图片位取图路径：none = 无封面（不渲染 img、不代理、不上报）。 */
export type CoverRoute = 'none' | 'direct' | 'proxy';

export type CoverFailReason = 'unavailable' | 'empty' | 'not-image' | 'error' | 'direct-error';

export type CoverState =
  | { status: 'idle' }
  | { status: 'loading' }
  | { status: 'ready'; src: string }
  | { status: 'failed'; reason: CoverFailReason };

/** 渲染决策：组件按它渲染 <img> 或占位，不自己再做判断。 */
export type CoverView =
  | { kind: 'none' }
  | { kind: 'img'; src: string; direct: boolean }
  | { kind: 'placeholder'; state: 'pending' | 'failed' };

const IDLE: CoverState = { status: 'idle' };
const LOADING: CoverState = { status: 'loading' };

/** 代理成功的 data: URL 缓存上限（防长时间滚动把大量 base64 常驻内存）；
    失败记录不受上限影响（它们很小，且淘汰会导致重复请求）。 */
const READY_CACHE_LIMIT = 300;

const states = new Map<string, CoverState>();
const readyOrder: string[] = [];
const listeners = new Set<() => void>();
const reported = new Set<string>();

function emit(): void {
  for (const fn of listeners) fn();
}

function setState(url: string, next: CoverState): void {
  states.set(url, next);
  if (next.status === 'ready') {
    /* G2（TASK-092 审查备忘）：push 前先摘除本 URL 可能残留的旧条目。正常流里
       驱逐（shift）那一刻已把它同步移出 readyOrder，重成功只会 push 一次；这里
       去重是把「驱逐 + 重成功不产生重复键」钉成结构不变量——一旦出现重复键，
       下面的 while 会把上限内的其他 ready 条目提前挤出（300 上限名存实亡）。 */
    const staleIdx = readyOrder.indexOf(url);
    if (staleIdx !== -1) readyOrder.splice(staleIdx, 1);
    readyOrder.push(url);
    while (readyOrder.length > READY_CACHE_LIMIT) {
      const old = readyOrder.shift();
      if (old !== undefined && states.get(old)?.status === 'ready') states.delete(old);
    }
  }
  emit();
}

/** 取图路径判定：空 → none；imageProxy 判定需要代理且当前有 IPC → proxy；其余直连。
    （无 IPC 的浏览器 mock 下与画廊同口径回退直连。） */
export function coverRoute(src: string | null | undefined): CoverRoute {
  if (!src || !src.trim()) return 'none';
  if (needsImageProxy(src) && canProxyImages()) return 'proxy';
  return 'direct';
}

export function getCoverState(url: string): CoverState {
  return states.get(url) ?? IDLE;
}

export function subscribeCover(fn: () => void): () => void {
  listeners.add(fn);
  return () => { listeners.delete(fn); };
}

/** 渲染决策（纯函数）：代理未返回前出占位（容器尺寸不变），失败出占位，成功/直连出 img。 */
export function coverView(src: string | null | undefined, state: CoverState): CoverView {
  const route = coverRoute(src);
  if (route === 'none' || !src) return { kind: 'none' };
  if (state.status === 'failed') return { kind: 'placeholder', state: 'failed' };
  if (route === 'proxy') {
    return state.status === 'ready'
      ? { kind: 'img', src: state.src, direct: false }
      : { kind: 'placeholder', state: 'pending' };
  }
  return { kind: 'img', src, direct: true };
}

/** 发起一次代理取图；同一 URL 在本会话内只发一次（进行中/已成功/已失败都直接返回）。 */
export function requestProxiedCover(url: string, pageUrl?: string): void {
  if (states.has(url)) return;
  setState(url, LOADING);
  void fetchProxiedImage(url, pageUrl).then((res) => {
    setState(url, res.ok ? { status: 'ready', src: res.dataUrl } : { status: 'failed', reason: res.reason });
  });
}

/** 直连 <img> 的 onError：记为失败（之后同一 URL 不再请求，直接出占位）。 */
export function markCoverFailed(url: string, reason: CoverFailReason = 'direct-error'): void {
  if (!url || getCoverState(url).status === 'failed') return;
  setState(url, { status: 'failed', reason });
}

/** 上报封面失效：同一条目同一 URL 只上报一次；无条目 id / 无 URL / data: URL 不上报。
    返回本次是否真的发起了 IPC。上报失败只 console.warn（不产生未处理的 rejection）。
    G3（TASK-092 审查备忘）：非数字 id 经 Number() 得 NaN 后在此静默拒绝是有意为之——
    前端条目 id 全部来自 String(row.id)（lib/api.ts articleRowToEntry）的纯数字串，
    转换无损；mock 会话的前缀 id 不该有封面失效上报（无后端可写），被 NaN 拦下
    与「无后端不上报」语义一致，不需要额外日志。 */
export function reportCoverFailure(articleId: string | null | undefined, url: string): boolean {
  if (!articleId || !url || url.startsWith('data:')) return false;
  const id = Number(articleId);
  if (!Number.isSafeInteger(id)) return false;
  const key = `${id}\n${url}`;
  if (reported.has(key)) return false;
  reported.add(key);
  void Promise.resolve()
    .then(() => api.reportBrokenCover(id, url))
    .catch((e: unknown) => { console.warn('[cover] report_broken_cover 失败（已忽略）', e); });
  return true;
}

/** 组件副作用的唯一入口：需要代理且未请求过 → 发起代理；已失败 → 上报（幂等）。 */
export function driveCover(src: string | null | undefined, pageUrl: string | undefined, articleId: string | null | undefined): void {
  const route = coverRoute(src);
  if (route === 'none' || !src) return;
  const st = getCoverState(src);
  if (route === 'proxy' && st.status === 'idle') requestProxiedCover(src, pageUrl);
  if (st.status === 'failed') reportCoverFailure(articleId, src);
}

/** 直连图片加载失败的回调（组件 onError 调用）。上报由状态变化后的 driveCover 统一发起。 */
export function onCoverError(src: string): void {
  markCoverFailed(src, 'direct-error');
}

/** 仅供测试：清空会话缓存与上报去重集合。 */
export function resetCoverCacheForTest(): void {
  states.clear();
  readyOrder.length = 0;
  reported.clear();
  emit();
}
