import { useEffect, type KeyboardEvent as ReactKeyboardEvent, type RefObject } from 'react';

// Note: 模态焦点陷阱契约与归因（含「为什么 trap 现在做」） — 见 .agents/notes/implemented/feature/2026-11-13-前端交互契约.md

const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

/** 面板内当前可聚焦元素：checkVisibility 可用时剔除 display:none 子树
    （真实浏览器里对不可见元素 focus() 是 no-op，会把循环焦点打进黑洞）；
    jsdom 无布局，checkVisibility 缺省或对可见元素返回 true → 不过滤。 */
function queryFocusable(panel: HTMLElement): HTMLElement[] {
  return Array.from(panel.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR))
    .filter(canRestoreFocus);
}

function canRestoreFocus(el: HTMLElement): boolean {
  return el.isConnected && !el.closest('[inert]')
    && (typeof el.checkVisibility !== 'function' || el.checkVisibility());
}

// 当前窗口的模态栈：下层先关闭或触发控件消失时，焦点仍归最上层存活面板。
const openPanels: HTMLElement[] = [];

/**
 * Tab 循环判定（纯逻辑，供浮层根元素的 keydown 调用）。
 * 返回是否已拦截（已移动焦点，调用方据此 preventDefault）。
 *
 * 边界语义（clamp，不跳面板外）：正向 Tab 在最后一个元素 → 回到第一个；
 * Shift+Tab 在第一个元素（或面板容器自身）→ 回到最后一个。面板内无任何
 * 可聚焦元素 → 焦点钉回面板容器（Tab 不逃逸）。焦点在面板 DOM 之外
 * （如 portal 下拉菜单，React 树内、DOM 树外）→ 不拦截，交还默认行为。
 */
export function trapModalTab(panel: HTMLElement | null, key: string, shiftKey: boolean): boolean {
  if (key !== 'Tab' || !panel || !panel.isConnected) return false;
  const active = document.activeElement;
  if (!(active instanceof Node) || !panel.contains(active)) return false;
  const focusables = queryFocusable(panel);
  if (focusables.length === 0) {
    panel.focus({ preventScroll: true });
    return true;
  }
  const first = focusables[0];
  const last = focusables[focusables.length - 1];
  const atEdge = shiftKey ? active === first || active === panel : active === last;
  if (!atEdge) return false;
  (shiftKey ? last : first).focus({ preventScroll: true });
  return true;
}

/**
 * 模态焦点陷阱：先捕获外部焦点，再移入 data-modal-autofocus 指定的控件
 * （未指定则面板容器）。输入不能用 commit 阶段的 autoFocus 抢先移焦。
 * 关闭/卸载时还原触发元素；触发点失效则回到仍打开的最上层面板。
 * 返回的 onTrapKeyDown 挂在浮层面板（或其根）上：React 合成事件沿组件树
 * 冒泡，portal 子树（FluxDropdown 菜单等）的 Tab 也能到达这里。
 */
export function useFocusTrap(open: boolean, panelRef: RefObject<HTMLElement | null>) {
  useEffect(() => {
    if (!open) return;
    const panel = panelRef.current;
    if (!panel) return;
    const restore = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    openPanels.push(panel);
    if (!(restore && panel.contains(restore))) {
      const initial = panel.querySelector<HTMLElement>('[data-modal-autofocus]');
      (initial ?? panel).focus({ preventScroll: true });
    }
    return () => {
      const index = openPanels.lastIndexOf(panel);
      if (index >= 0) openPanels.splice(index, 1);
      const top = openPanels.findLast((el) => canRestoreFocus(el));
      if (restore && canRestoreFocus(restore) && !panel.contains(restore)
        && (!top || top.contains(restore))) {
        restore.focus({ preventScroll: true });
      } else {
        top?.focus({ preventScroll: true });
      }
    };
  }, [open, panelRef]);

  const onTrapKeyDown = (e: ReactKeyboardEvent) => {
    if (trapModalTab(panelRef.current, e.key, e.shiftKey)) {
      e.preventDefault();
    }
  };
  return { onTrapKeyDown };
}
