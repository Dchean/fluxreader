import { useLayoutEffect, type RefObject } from 'react';

/**
 * 一次性入场动画类：`key` 变化时给元素挂上 `className`，动画结束后摘掉。
 *
 * 类名不能一直挂着：CSS 动画只在类名被新挂上时开始，切换内容时不会重播。
 * 所以 `key` 变化后要先摘掉、强制一次重排、再挂上——同一帧内摘了又挂不会重放，
 * 中间那次重排是必需的。
 *
 * 用 useLayoutEffect 在首次绘制前挂类名，避免首帧先显示终态再淡入；直接操作
 * DOM 而不是走 state，切换列表时不会为此多渲染一次。
 */
export function useEnteringClass<T extends HTMLElement>(
  ref: RefObject<T | null>,
  key: string,
  className: string,
): void {
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.classList.remove(className);
    void el.offsetWidth;
    el.classList.add(className);
    const clear = (event: AnimationEvent) => {
      // animationend 会冒泡：子孙元素的动画结束不该摘掉宿主自身的入场类。
      if (event.target !== el) return;
      el.classList.remove(className);
    };
    el.addEventListener('animationend', clear);
    return () => {
      el.removeEventListener('animationend', clear);
      el.classList.remove(className);
    };
  }, [ref, key, className]);
}
