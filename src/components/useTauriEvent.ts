import { useEffect, useRef } from 'react';

/**
 * Tauri 事件订阅 hook：组件生命周期内订阅一个事件，卸载时退订。
 *
 * 旧模式的竞态：`let unlisten; void (async () => { unlisten = await listen(...) })();
 * return () => unlisten?.()`——cleanup 先于 listen 完成发生时（快速挂载/卸载、
 * StrictMode 双挂载），cleanup 读到的 unlisten 还是 undefined，注册完成后
 * 再也无人退订：监听器泄漏，组件卸载后 handler 仍会被触发。
 * 现以 disposed 标志补口：注册完成时若已卸载，立即自摘除。
 */
export function useTauriEvent<T>(event: string, handler: (payload: T) => void): void {
  /* handler 经 ref 转发：事件回调永远消费最新一次渲染的闭包，而订阅本身
     不随 handler 身份重建（handler 内部一律走 useAppStore.getState()）。 */
  const handlerRef = useRef(handler);
  useEffect(() => {
    handlerRef.current = handler;
  });

  useEffect(() => {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const { listen } = await import('@tauri-apps/api/event');
        const fn = await listen<T>(event, (e) => handlerRef.current(e.payload));
        if (disposed) {
          fn();
          return;
        }
        unlisten = fn;
      } catch {
        /* 事件监听失败不影响主流程 */
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
      unlisten = undefined;
    };
  }, [event]);
}
