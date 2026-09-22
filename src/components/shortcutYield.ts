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
   浮层集合本身由 OVERLAY_SOURCES 清单定义、anyOverlayOpen 求值，
   使「新增/漏判浮层」成为可断言项（见下方该清单的说明）。

   修前行为（TASK-081 之前）：S/M/J/K 不看浮层状态 —— 设置页、搜索框、各类弹窗打开时，
   焦点若不在输入框上，按 S/M 会作用到**浮层背后的当前文章**（改了它的收藏/已读却看不见），
   J/K 还会在背后切换文章。
   ============================================================ */

/** 会因浮层打开而让路的单键（大小写都算）。 */
export const OVERLAY_YIELD_KEYS: readonly string[] = ['s', 'S', 'm', 'M', 'j', 'k'];

/* ============================================================
   浮层集合的**唯一定义**。

   为什么要抽成一个显式清单、而不是把 `a || b || c` 内联在 App.tsx：
   内联写法下，「少判一个浮层」是一个**能通过全部断言**的真实回归——
   回归网只能看到源码里出现了某个 token，看不出这个并集少了一项
   （实测：把「全屏播放器」从 overlayOpen 移除，322 条断言全绿）。
   改为按此清单求值后，清单本身可被逐项钉死，新增浮层也必须同步清单，
   与 OVERLAY_YIELD_KEYS 的既有做法一致。 */
export interface OverlayState {
  searchOpen: boolean;
  settingsOpen: boolean;
  newCategoryModalOpen: boolean;
  addFeedModalOpen: boolean;
  editFeedModalOpen: boolean;
  renameCatModalOpen: boolean;
  lightboxUrl: string | null;
  playerExpanded: boolean;
  playerActive: boolean;
}

/** 浮层清单：每项给出「名称 + 该浮层当前是否打开」。新增浮层必须在此登记。 */
export const OVERLAY_SOURCES: readonly {
  name: string;
  isOpen: (s: OverlayState) => boolean;
}[] = [
  { name: 'search', isOpen: (s) => s.searchOpen },
  { name: 'settings', isOpen: (s) => s.settingsOpen },
  { name: 'newCategory', isOpen: (s) => s.newCategoryModalOpen },
  { name: 'addFeed', isOpen: (s) => s.addFeedModalOpen },
  { name: 'editFeed', isOpen: (s) => s.editFeedModalOpen },
  { name: 'renameCat', isOpen: (s) => s.renameCatModalOpen },
  { name: 'lightbox', isOpen: (s) => !!s.lightboxUrl },
  /* Full Player 大浮层：仅在播放器激活时才算浮层 */
  { name: 'playerExpanded', isOpen: (s) => s.playerExpanded && s.playerActive },
];

/** 任一浮层打开 ⇒ true。App.tsx 直接调用本函数，回归网对同一份清单逐项断言。 */
export function anyOverlayOpen(s: OverlayState): boolean {
  return OVERLAY_SOURCES.some((o) => o.isOpen(s));
}

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
