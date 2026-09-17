/* ============================================================
   列表哨兵（滚动加载指示）的可见性判定 —— 纯函数，独立成文件。

   为什么独立成文件：它被 src/components/Timeline.tsx 消费，同时被回归网
   （tools/frontend-regression.mjs）直接断言。组件文件里 export 非组件函数会
   触发 oxlint 的 react/only-export-components（Fast Refresh 约束），因此按
   该规则的建议放到本文件，两侧共用同一份判定，避免出现「断言一套、渲染一套」。

   契约（TASK-052）：
   - 列表非空：始终渲染（loading / end / idle 三态）；
   - 列表为空但**未到底**：仍渲染 idle——修前这里被 `items.length > 0` 挡住，
     空列表没有哨兵 ⇒ 无滚动 ⇒ 该范围的老文章永远够不到（缺陷 P1-14 之一）；
   - 列表为空且已到底：不渲染，避免「没有更多了」与「暂无匹配内容」重复。
   ============================================================ */

export type SentinelMode = 'hidden' | 'loading' | 'end' | 'idle';

export function sentinelMode(itemCount: number, exhausted: boolean, loading: boolean): SentinelMode {
  if (itemCount === 0 && exhausted) return 'hidden';
  if (loading) return 'loading';
  if (exhausted) return 'end';
  return 'idle';
}
