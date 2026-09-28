/* ============================================================
   列表「不足一屏自动续拉」的判定 —— 纯函数，独立成文件。

   为什么独立成文件：与 timelineSentinel.ts 同一考虑——它被 Timeline.tsx 消费，
   同时被回归网（tools/frontend-regression.mjs）直接断言；组件文件里 export
   非组件函数会触发 oxlint 的 react/only-export-components（Fast Refresh 约束），
   两侧共用同一份判定，避免「断言一套、渲染一套」。

   背景（REQ-107 / TASK-094）：基线 s7b B1 实测画廊 5/44、播客 6/119、通知 1/20
   且容器 scrollHeight==clientHeight 不可滚动——后端按全局分页、前端按布局本地
   过滤时，稀疏布局的首批只剩几条，onScroll 永不触发，分页停在首批。列表查询加
   布局维度后首批即含该布局全部条目，但「全部」视图的可见集合仍会被未读等筛选
   筛到不足一屏——onScroll 依旧不会触发。TASK-052 的空列表补拉只在 items==0 时
   生效；本模块把补拉判定推广到「列表非空但未撑满视口」，并加**连续调用上限**
   防止「后端持续返回整页新数据、可见集合永不增长」时的死循环——上限内仍取不到
   就停手，交给人肉「加载更多」按钮（Timeline 哨兵的不可滚动分支）。
   ============================================================ */

export type RefillDecision = 'refill' | 'idle';

/** 自动续拉的连续调用上限：每次调用最多拉一页（ARTICLES_PAGE_SIZE=500 行），
    连续上限次仍无可见进展即停止自动补拉。只统计「自动」路径；用户点击
    「加载更多」按钮不经过本判定。可见进展（items 增长）或筛选口径变化会重置计数。 */
export const AUTO_REFILL_MAX_CALLS = 8;

export function refillDecision(args: {
  itemCount: number;
  exhausted: boolean;
  loading: boolean;
  /** 容器内容已撑满视口（可滚动）：scrollHeight > clientHeight */
  filledViewport: boolean;
  autoCalls: number;
  cap?: number;
}): RefillDecision {
  const cap = args.cap ?? AUTO_REFILL_MAX_CALLS;
  if (args.loading || args.exhausted) return 'idle'; // 在途 / 已到底：入口守卫语义一致
  if (args.autoCalls >= cap) return 'idle'; // 上限：连续无进展即停，交给手动按钮
  if (args.itemCount === 0) return 'refill'; // TASK-052 空列表补拉（口径并入同一上限）
  if (!args.filledViewport) return 'refill'; // TASK-094：非空但未撑满视口 → 续拉
  return 'idle';
}
