/* ============================================================
   外链打开：统一走 Tauri opener 插件（系统默认浏览器）；
   浏览器开发环境降级 window.open。正文内 <a> 点击拦截也走这里。
   ============================================================ */

import { EXTERNAL_OPEN_FAILURE } from './copyPolicy';

// Note: 完成态正文/译文共用单次外开与协议拒绝边界 — 见 .agents/notes/implemented/architecture/2026-11-05-主页面导航与外链边界.md

let openUrlFn: ((url: string) => Promise<void>) | null = null;
const OPEN_FAILURE_HANDLER_KEY = '__FLUXREADER_EXTERNAL_OPEN_FAILURE__';

/** 由 store 注入 toast 单点；经 globalThis 共享，避免测试/生产出现第二份模块实例。 */
export function bindExternalOpenFailureHandler(handler: (message: string) => void): void {
  (globalThis as Record<string, unknown>)[OPEN_FAILURE_HANDLER_KEY] = handler;
}

function reportOpenFailure(message: string): void {
  const handler = (globalThis as Record<string, unknown>)[OPEN_FAILURE_HANDLER_KEY];
  if (typeof handler === 'function') (handler as (text: string) => void)(message);
}

/** 懒加载 opener（浏览器环境无 Tauri IPC，动态 import 不会进 bundle） */
async function getOpener(): Promise<((url: string) => Promise<void>) | null> {
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return null;
  if (openUrlFn) return openUrlFn;
  try {
    const { openUrl } = await import('@tauri-apps/plugin-opener');
    openUrlFn = (url: string) => openUrl(url);
    return openUrlFn;
  } catch {
    return null;
  }
}

async function openExternalRaw(url: string | null | undefined): Promise<void> {
  const target = (url ?? '').trim();
  const parsed = new URL(target);
  if (!/^https?:$/.test(parsed.protocol)) throw new Error('仅支持 HTTP/HTTPS 链接');
  const opener = await getOpener();
  if (opener) {
    await opener(target);
    return;
  }
  if (!window.open(target, '_blank', 'noopener,noreferrer')) {
    throw new Error('浏览器未接受打开请求');
  }
}

type OpenExternalOptions = {
  reportFailure?: boolean;
  rethrow?: boolean;
};

/** 所有用户入口的唯一封装：失败统一提示；需要保留原始错误语义的流程可 rethrow。 */
export async function openExternalWithFeedback(
  url: string | null | undefined,
  options: OpenExternalOptions = {},
): Promise<boolean> {
  try {
    await openExternalRaw(url);
    return true;
  } catch (error) {
    if (options.reportFailure !== false) reportOpenFailure(EXTERNAL_OPEN_FAILURE);
    if (options.rethrow) throw error;
    return false;
  }
}

/**
 * 正文容器点击代理：拦截 <a> 点击走 opener（正文 HTML 来自外部源，
 * 默认 target 不可控，统一交给系统浏览器）。
 * 在正文容器 onClick 调用：`onClick={(e) => handleArticleLinkClick(e)}`。
 */
export function handleArticleLinkClick(e: React.MouseEvent<HTMLDivElement>): void {
  if (e.defaultPrevented) return;
  const anchor = (e.target as Element).closest?.('a[href]');
  if (!anchor || !e.currentTarget.contains(anchor)) return;
  const href = anchor.getAttribute('href');
  e.preventDefault();
  void openExternalWithFeedback(href);
}
