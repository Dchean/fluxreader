// Note: J/K 覆盖全部五种布局，选中与 DOM 焦点由 Timeline 单点同步 — 见 .agents/notes/implemented/architecture/2026-11-04-全布局键盘焦点与响应式阅读列.md
import type { ContentLayoutType } from '../types';

/* ============================================================
   TASK-114 X3：J/K 布局门控与选中推进 —— 纯函数，独立成文件。

   为什么独立成文件：它被 src/App.tsx 的 keydown J/K 分支消费，同时被
   回归网（tools/frontend-regression.mjs）直接断言（真值表 + store 层
   五布局模拟）。内联在 App.tsx 的写法「改了行为却无法断言」（回归网只能
   扫源码 token），故沿用 shortcutYield.ts / timelineSentinel.ts /
   store/selectors.ts 的既有做法：判定收口到纯函数，两侧共用同一份。

   契约（UI-CONTRACT-TASK-114-LAYOUT-CONSISTENCY.md X3）：
   - J/K 选中切换（循环回绕）覆盖 article/social/image/podcast/notification；
   - Timeline 在 activeArticleId 变化后执行 scrollToIndex + DOM focus，
     画廊与四类单列布局共用同一选中/焦点/可见态；
   - 输入框守卫（inInput）与浮层让路（shouldYieldToOverlay）规则不变。
   ============================================================ */

/**
 * J/K 布局门控：五种现有布局全部放行。
 *
 * 保留纯函数作为 App 与回归网的共同契约；新增布局时必须显式决定其键盘
 * 语义，不再依赖「排除某一布局」的隐式继承。
 */
export function jkLayoutAllowed(layout: ContentLayoutType): boolean {
  switch (layout) {
    case 'article':
    case 'social':
    case 'image':
    case 'podcast':
    case 'notification':
      return true;
    default:
      return false;
  }
}

/**
 * J/K 选中推进（循环回绕），语义与修前 App.tsx 内联实现逐条等价：
 * - 空列表（itemCount<=0）→ -1，调用方跳过（不选中任何条目）；
 * - 当前无选中（curIdx<0）→ j 落首项、k 落末项（两端入场）；
 * - j（forward=true）：向下一位，末项回绕到 0；
 * - k（forward=false）：向上一位，首项回绕到末项。
 * 返回目标下标；调用方按下标取 selectVisibleEntries 的条目后 selectArticle。
 */
export function jkNextIndex(itemCount: number, curIdx: number, forward: boolean): number {
  if (itemCount <= 0) return -1;
  if (curIdx < 0) return forward ? 0 : itemCount - 1;
  if (forward) return curIdx < itemCount - 1 ? curIdx + 1 : 0;
  return curIdx > 0 ? curIdx - 1 : itemCount - 1;
}
