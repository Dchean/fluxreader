/* ============================================================
   命令面板「锚定打开文章」的前置导航归一 —— 纯函数，独立成文件。

   为什么独立成文件：它被 src/components/Overlays.tsx 的「文章」命令项消费，
   同时被回归网（tools/frontend-regression.mjs）直接断言。组件文件里 export
   非组件函数会触发 oxlint 的 react/only-export-components（Fast Refresh 约束），
   故放到本文件，组件与断言共用同一份定义，避免「断言一套、组件一套」。

   搜索结果可以来自当前范围/视图之外。打开前先将导航归一到可捕获目标的阅读范围，
   再建立或复用当前 ReadingSession；若目标不属于捕获成员，Reader 仍可按文章 ID
   注入详情行而不改变会话列表或加载进度。导航若放在打开之后，后续新会话会替换
   目标上下文，破坏搜索打开动作与当前列表之间的关系。
   ============================================================ */

import type { ViewFilterType } from '../types';

/** 锚定前需要执行的导航步骤（数组序 = 执行序）。 */
export type AnchorScopeNavStep = {
  action: 'selectFeed' | 'selectView' | 'toggleTimelineFilter';
  arg?: string;
};

/** 把当前导航状态归一成 anchorToArticle 需要的「全部范围 × 全部视图 × 非未读筛选」。
    返回依次执行的导航动作；已在目标态时返回空数组（幂等，不产生多余切换）。 */
export function anchorScopeNav(state: {
  activeFeedFilter: string;
  activeViewFilter: ViewFilterType;
  timelineFilter: string;
}): AnchorScopeNavStep[] {
  const nav: AnchorScopeNavStep[] = [];
  // 统一在「全部订阅源」范围锚定（搜索结果可能来自任意 feed）
  if (state.activeFeedFilter !== 'all') nav.push({ action: 'selectFeed', arg: 'all' });
  // 未读/今天等视图会过滤掉目标文章，先切到「全部」视图
  if (state.activeViewFilter !== 'all') nav.push({ action: 'selectView', arg: 'all' });
  if (state.timelineFilter === 'unread') nav.push({ action: 'toggleTimelineFilter' });
  return nav;
}
