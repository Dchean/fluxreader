import type { ContentLayoutType, ViewFilterType } from '../types';

/** 查询输入只在用户建立新范围时快照；跨零点不会改写正在阅读的会话。 */
export interface ReadingQuery {
  layout: ContentLayoutType;
  view: ViewFilterType;
  scope: string;
  filter: 'all' | 'unread';
  sort: 'newest' | 'oldest';
  dayStartMs: number;
  dayEndMs: number;
}

/** 成员与排序由本会话拥有；读藏、来源和正文更新属于实体属性，不重新筛成员。 */
export interface ReadingSession {
  generation: number;
  query: ReadingQuery;
  ready: boolean;
  /** 初始筛选的完整轻量 ID 集合；正文和行属性仍只按窗口回读。 */
  memberIds: readonly string[];
  loadedCount: number;
  windowStart: number;
}

export type ReadingNavigation = Pick<ReadingQuery, 'layout' | 'view' | 'scope' | 'filter' | 'sort'>;

/** 同一用户范围的比较不含日界：日期变化只影响下一次显式重筛。 */
export function sameReadingNavigation(a: ReadingNavigation, b: ReadingNavigation): boolean {
  return a.layout === b.layout && a.view === b.view && a.scope === b.scope
    && a.filter === b.filter && a.sort === b.sort;
}

/** 请求/缓存身份含实际日期边界，今天的旧查询不能给新会话提供成员。 */
export function readingQueryKey(query: ReadingQuery): string {
  return JSON.stringify([
    query.layout, query.view, query.scope, query.filter, query.sort,
    ...(query.view === 'today' ? [query.dayStartMs, query.dayEndMs] : []),
  ]);
}

export function createReadingSession(generation: number, query: ReadingQuery, memberIds: readonly string[] = []): ReadingSession {
  return { generation, query: { ...query }, ready: false, memberIds: [...new Set(memberIds)], loadedCount: 0, windowStart: 0 };
}

export function captureReadingQuery(navigation: ReadingNavigation, now = Date.now()): ReadingQuery {
  return { ...navigation, ...localDayBounds(now) };
}

export function localDayBounds(now = Date.now()): Pick<ReadingQuery, 'dayStartMs' | 'dayEndMs'> {
  const start = new Date(now);
  start.setHours(0, 0, 0, 0);
  const end = new Date(start);
  end.setDate(end.getDate() + 1);
  return { dayStartMs: start.getTime(), dayEndMs: end.getTime() };
}

/** 新范围的首屏和同会话续页共用响应身份；A→B→A 不能仅比较查询字符串。 */
export function acceptsReadingResponse(session: ReadingSession, generation: number, query: ReadingQuery): boolean {
  return session.generation === generation && readingQueryKey(session.query) === readingQueryKey(query);
}

/** 回读仍未载入的既定成员，不再向“当前未读/收藏”查询续页，避免尾部被同步抽走。 */
export function nextReadingPage(session: ReadingSession, pageSize: number): readonly string[] {
  return session.memberIds.slice(session.loadedCount, session.loadedCount + pageSize);
}

/** 缺行也消费其成员位置；调用方保留缺行占位，不能反复请求同一页或静默挪动后续项。 */
export function commitReadingPage(session: ReadingSession, generation: number, query: ReadingQuery, memberCount: number): ReadingSession {
  if (!acceptsReadingResponse(session, generation, query)) return session;
  const loadedCount = Math.min(session.memberIds.length, session.loadedCount + memberCount);
  return loadedCount !== session.loadedCount ? { ...session, loadedCount } : session;
}

/** 后台属性回读只更换同身份实体；不插入新成员、不删除缺行，也不按属性再排序。
 * 明确删除的占位由调用方提供；普通空响应不能伪造删除。 */
export function projectReadingMembers<T extends { id: string }>(
  session: ReadingSession,
  previous: readonly T[],
  updates: readonly T[],
): T[] {
  const previousById = new Map(previous.map((entry) => [entry.id, entry]));
  const updatesById = new Map(updates.map((entry) => [entry.id, entry]));
  return session.memberIds.slice(session.windowStart, session.loadedCount).flatMap((id) => {
    const entry = updatesById.get(id) ?? previousById.get(id);
    return entry ? [entry] : [];
  });
}
