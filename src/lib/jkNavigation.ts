// Note: J/K 门控用「排除 image」而非白名单，新增虚拟化布局自动获得 — 见 .agents/notes/implemented/feature/2026-10-06-键盘导航、焦点与卡片角色.md
import type { ContentLayoutType } from '../types';

/* ============================================================
   TASK-114 X3：J/K 布局门控与选中推进 —— 纯函数，独立成文件。

   为什么独立成文件：它被 src/App.tsx 的 keydown J/K 分支消费，同时被
   回归网（tools/frontend-regression.mjs）直接断言（真值表 + store 层
   五布局模拟）。内联在 App.tsx 的写法「改了行为却无法断言」（回归网只能
   扫源码 token），故沿用 shortcutYield.ts / timelineSentinel.ts /
   store/selectors.ts 的既有做法：判定收口到纯函数，两侧共用同一份。

   契约（UI-CONTRACT-TASK-114-LAYOUT-CONSISTENCY.md X3）：
   - J/K 选中切换（循环回绕）从「仅 article」扩展到全部虚拟化布局
     （article/social/podcast/notification，复用 Timeline 的
     moveCardFocus/focusIndex 基建与 focusIndex 跟随选中 effect）；
   - 画廊（image）非虚拟化、无 moveCardFocus 基建：不支持 J/K；
   - 输入框守卫（inInput）与浮层让路（shouldYieldToOverlay）规则不变。
   ============================================================ */

/**
 * J/K 布局门控：image 之外全部放行。
 *
 * 用「排除 image」而非四布局白名单：与 Timeline 虚拟化开关
 * （enabled: activeContentLayout !== 'image'）保持同一口径——未来新增
 * 虚拟化布局自动获得 J/K，无需同步本处；画廊的非虚拟化才是「不支持」的
 * 根因（没有 data-card-index 定位与 scrollToIndex 定位基建）。
 */
export function jkLayoutAllowed(layout: ContentLayoutType): boolean {
  return layout !== 'image';
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
