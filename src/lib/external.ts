/* ============================================================
   外链打开：统一走 Tauri opener 插件（系统默认浏览器）；
   浏览器开发环境降级 window.open。正文内 <a> 点击拦截也走这里。
   ============================================================ */

let openUrlFn: ((url: string) => Promise<void>) | null = null;

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

/** 打开外部 URL（文章源网页/查看原文/正文内链接） */
export async function openExternal(url: string | null | undefined): Promise<void> {
  const target = (url ?? '').trim();
  if (!target) return;
  /* 只放行 http/https，防止外部内容注入 file:// 或自定义协议 */
  if (!/^https?:\/\//i.test(target)) return;
  const opener = await getOpener();
  if (opener) {
    await opener(target);
    return;
  }
  window.open(target, '_blank', 'noopener,noreferrer');
}

/**
 * 正文容器点击代理：拦截 <a> 点击走 opener（正文 HTML 来自外部源，
 * 默认 target 不可控，统一交给系统浏览器）。
 * 在正文容器 onClick 调用：`onClick={(e) => handleArticleLinkClick(e)}`。
 */
export function handleArticleLinkClick(e: React.MouseEvent<HTMLDivElement>): void {
  const anchor = (e.target as HTMLElement).closest('a');
  if (!anchor) return;
  const href = anchor.getAttribute('href');
  if (!href) return;
  e.preventDefault();
  void openExternal(href);
}
