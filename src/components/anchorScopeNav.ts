/* ============================================================
   命令面板「锚定打开文章」的前置导航归一 —— 纯函数，独立成文件。

   为什么独立成文件：它被 src/components/Overlays.tsx 的「文章」命令项消费，
   同时被回归网（tools/frontend-regression.mjs）直接断言。组件文件里 export
   非组件函数会触发 oxlint 的 react/only-export-components（Fast Refresh 约束），
   故放到本文件，组件与断言共用同一份定义，避免「断言一套、组件一套」。

   【TASK-052 行为变化】为什么必须先归一、再锚定：
   anchorToArticle 按**调用时**的 activeFeedFilter / activeViewFilter /
   timelineFilter 构造 article_index 与 list_articles 的查询参数（与
   loadMoreArticles 共用 internals.scopeQueryArgs，per-scope 口径）。因此前置
   导航若在锚定**之后**执行，锚定算出的「绝对位置」是**旧范围**下的位置，而它随后
   加载的那一页要放进新范围的列表 —— 位置与该位置的列表不同口径，锚定错位；更糟的
   情形是目标文章根本不在旧范围的筛选结果内（article_index 返回 null），锚定静默失败、
   文章打不开。
   改造前分页/锚定查询都不带范围，两种顺序结果相同，所以旧顺序看不出问题；是
   per-scope 让这个顺序开始出错，故本修正属于「完成口径改造」而非新功能。
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
