// ReadingSession 纯成员合同；独立执行使用既有 tools/ui-loader.mjs 编译源模块。
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createReadingSession, nextReadingPage, commitReadingPage, projectReadingMembers, sameReadingNavigation, readingQueryKey } from '../../src/store/readingSession.ts';

const query = { layout: 'article', view: 'unread', scope: 'all', filter: 'all', sort: 'newest', dayStartMs: 0, dayEndMs: 86400000 };

test('会话成员在标读、取消收藏、后台插入与时间改写时仍保持原顺序', () => {
  const session = commitReadingPage(createReadingSession(1, query, ['a', 'b']), 1, query, 2);
  const previous = [{ id: 'a', read: false, starred: true, published: 20 }, { id: 'b', read: false, starred: true, published: 10 }];
  const projected = projectReadingMembers(session, previous, [
    { id: 'b', read: false, starred: false, published: 30 },
    { id: 'new', read: false, starred: true, published: 40 },
    { id: 'a', read: true, starred: true, published: 20 },
  ]);
  assert.deepEqual(projected.map((entry) => entry.id), ['a', 'b']);
  assert.equal(projected[0].read, true);
  assert.equal(projected[1].starred, false);
  assert.deepEqual(projectReadingMembers(session, projected, []), projected, '查询未返回不等于条目被删除');
});

test('续页按初始全成员切片，尾部最新属性变化仍可到达；旧代际不能推进新会话', () => {
  const session = createReadingSession(3, query, ['a', 'b', 'c', 'c']);
  assert.deepEqual(nextReadingPage(session, 2), ['a', 'b']);
  assert.equal(commitReadingPage(session, 1, query, 2), session);
  assert.equal(commitReadingPage(session, 3, { ...query, scope: 'other' }, 2), session);
  const first = commitReadingPage(session, 3, query, 2);
  assert.deepEqual(nextReadingPage(first, 2), ['c'], '尾部c即使已被其它端标读/取消收藏，仍在固定成员分页');
  const next = commitReadingPage(first, 3, query, 1);
  assert.deepEqual(projectReadingMembers(next, [{ id: 'a' }, { id: 'b' }], [{ id: 'c', read: true }]).map((item) => item.id), ['a', 'b', 'c']);
  assert.equal(session.loadedCount, 0, '分页进度不修改旧会话快照');
  assert.deepEqual(nextReadingPage(next, 2), []);
});

test('跨零点不改变当前导航会话，重新进入今天时查询身份包含新日界', () => {
  const today = { ...query, view: 'today' };
  const tomorrow = { ...today, dayStartMs: 86400000, dayEndMs: 172800000 };
  assert.equal(sameReadingNavigation(today, tomorrow), true);
  assert.notEqual(readingQueryKey(today), readingQueryKey(tomorrow));
  const session = createReadingSession(1, today, ['yesterday']);
  assert.equal(commitReadingPage(session, 1, tomorrow, 1), session);
  assert.deepEqual(session.memberIds, ['yesterday']);
});
