/* ============================================================
   「滚出列表上方 ⇒ 标已读」的判定 —— 纯函数，独立成文件。

   为什么独立成文件：它被 src/components/Timeline.tsx 的滚动效应消费，同时被回归网
   （tools/frontend-regression.mjs）直接断言。组件文件里 export 非组件函数会触发
   oxlint 的 react/only-export-components（Fast Refresh 约束），因此按该规则的建议
   放到本文件（与 shortcutYield.ts / timelineSentinel.ts 同一做法），两侧共用同一份
   判定，避免「断言一套、运行一套」。

   缺陷背景（AUDIT-20260919-v2 的 F5「Timeline 误标已读窗口」，REQ-102）：
   切换 布局/视图/订阅范围/筛选/排序 会换掉整个 items 序列，并触发列表
   `scrollTo({ top: 0 })`。但归零是**异步生效**的：在它生效前，滚动效应仍可能读到
   **切换前的** range.startIndex（如切到 image 布局时虚拟化被禁用，range 保留旧值）。
   此时「start 大于上次基准」成立，循环会把**新序列里** index 0..start 的新条目
   ——用户从未见过的——整段标成已读。

   影响面与「旧 startIndex / 新序列长度」正相关：列表越长越容易命中；归零生效后窗口
   即关闭，因此表现为「小概率误标已读」——这正是该缺陷此前难以复现的原因。

   契约：仅当以下两条同时成立才认为条目「滚出上方」：
   1. 本次 startIndex 变化确由**用户滚动**引起（程序性归零/换序列不算，
      故 scrollToIndex 定位与筛选切换都不会触发标读）；
   2. startIndex 相对基准**递增**。
   ============================================================ */

export interface ScrollAwayInput {
  /** 本次判定是否由用户滚动引起（筛选切换/程序性滚动为 false） */
  scrollDriven: boolean;
  /** 虚拟列表当前可视区起始 index */
  startIndex: number;
  /** 上次记录的起始 index 基准 */
  lastStartIndex: number;
  /** 当前条目总数（用于夹取上界） */
  itemCount: number;
}

export interface ScrollAwayDecision {
  /** 需要标为已读的 index 区间 [from, to)；为空表示本次不标读 */
  range: { from: number; to: number } | null;
  /** 本次判定后应写入的新基准（始终等于夹取后的 startIndex） */
  nextLastStartIndex: number;
}

/** 「滚出列表上方」的条目 index 区间；不可判定时为 null。 */
export function scrollAwayRange(input: ScrollAwayInput): ScrollAwayDecision {
  const { scrollDriven, lastStartIndex, itemCount } = input;
  /* index 夹取到 [0, itemCount]：虚拟列表在数据切换瞬间可能给出越界 index */
  const start = Math.max(0, Math.min(input.startIndex, itemCount));
  const last = Math.max(0, Math.min(lastStartIndex, itemCount));
  /* 非用户滚动：只对齐基准、绝不标读（换序列的异步窗口就走这支） */
  if (!scrollDriven) return { range: null, nextLastStartIndex: start };
  if (start <= last) return { range: null, nextLastStartIndex: start };
  return { range: { from: last, to: start }, nextLastStartIndex: start };
}
