/* ============================================================
   全局单键快捷键「是否让路浮层」的判定 —— 纯函数，独立成文件。

   为什么独立成文件：它被 src/App.tsx 的 keydown 分支消费，同时被回归网
   （tools/frontend-regression.mjs）直接断言。组件文件里 export 非组件函数会触发
   oxlint 的 react/only-export-components（Fast Refresh 约束），因此按该规则的建议
   放到本文件（与 timelineSentinel.ts / store/selectors.ts 同一做法），两侧共用同一份
   判定，避免「断言一套、渲染一套」。

   契约（P3[F2] / REQ-104 / TASK-086）：
   - 任一浮层打开（搜索/设置/新建分类/添加订阅/编辑订阅/重命名分类/图片 lightbox/
     全屏播放器）时，**单键**快捷键 S/M/J/K 让路 → 'yield'；
   - 带 Ctrl / Meta / Alt 的组合键**不让路**（Ctrl+K、Ctrl+, 属浮层自身操作，
     在 App.tsx 中位于本判定之前处理）；
   - 非 S/M/J/K 键不让路；
   - 浮层未打开时不让路。

   修前行为（TASK-081 之前）：S/M/J/K 不看浮层状态 —— 设置页、搜索框、各类弹窗打开时，
   焦点若不在输入框上，按 S/M 会作用到**浮层背后的当前文章**（改了它的收藏/已读却看不见），
   J/K 还会在背后切换文章。
   ============================================================ */

/** 会因浮层打开而让路的单键（大小写都算）。 */
export const OVERLAY_YIELD_KEYS: readonly string[] = ['s', 'S', 'm', 'M', 'j', 'k'];

export type ShortcutOutcome = 'yield' | 'proceed';

export function shouldYieldToOverlay(
  overlayOpen: boolean,
  key: string,
  hasModifier: boolean,
): ShortcutOutcome {
  if (!overlayOpen) return 'proceed';
  if (hasModifier) return 'proceed';
  return OVERLAY_YIELD_KEYS.includes(key) ? 'yield' : 'proceed';
}
