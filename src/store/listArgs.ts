import type { ArticleListArgs } from '../lib/api';
import { numericId, resolveFeedLayout } from './selectors';
import type { AppState } from './types';

/**
 * 从当前导航状态构造后端列表/「全部已读」共用的筛选参数（I-UI-1 / I-DATA-3）。
 * 口径与 `selectVisibleEntries` / `selectViewCounts` 完全一致：
 * 订阅范围（activeFeedFilter）× 视图（activeViewFilter）× 时间流（timelineFilter）
 * × 内容布局（activeContentLayout → 该布局下的 feed 集合）。
 *
 * `offset`/`limit`/`with_content` 由调用方补充（列表分页 vs 全部已读无需分页）。
 */
export function buildListArgs(
  s: Pick<
    AppState,
    'activeContentLayout' | 'activeViewFilter' | 'activeFeedFilter' | 'timelineFilter' | 'feedIndex'
  >,
  opts: { offset?: number; limit?: number; with_content?: boolean } = {},
): ArticleListArgs {
  // 订阅范围 → feed_id / folder_id（activeFeedFilter 三种形态：all / cat-xxx / feed-xxx）
  const scope = s.activeFeedFilter;
  const feedId = scope.startsWith('cat-') ? null : scope === 'all' ? null : numericId(scope);
  const folderId = scope.startsWith('cat-') ? numericId(scope) : null;

  // 视图筛选：与 selectVisibleEntries 同口径——today/unread/starred 各自语义；
  // 「显示:未读」只在「全部」视图下生效。
  const view = s.activeViewFilter;
  const only_unread = view === 'unread' || (view === 'all' && s.timelineFilter === 'unread') || undefined;
  const only_starred = view === 'starred' || undefined;
  const only_today = view === 'today' || undefined;

  // 当前内容布局下的 feed 集合：布局是「feed 绑定 → 分类布局」的动态解析结果，
  // 全部已读必须限定到该布局下的源，否则会误标其他布局的文章。
  const feedIds: number[] = [];
  for (const [feedIdStr, binding] of s.feedIndex) {
    if (resolveFeedLayout(binding.feed, binding.cat.layout) === s.activeContentLayout) {
      const id = numericId(feedIdStr);
      if (Number.isFinite(id)) feedIds.push(id);
    }
  }
  const feed_ids = feedIds.length ? feedIds : [];

  return {
    feed_id: feedId || null,
    folder_id: folderId || null,
    feed_ids: feed_ids.length ? feed_ids : [],
    only_unread,
    only_starred,
    only_today,
    newest_first: true,
    limit: opts.limit,
    offset: opts.offset,
    with_content: opts.with_content,
  };
}
