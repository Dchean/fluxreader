// AF1 R2：字段投影必须跨活跃窗口/所有现存缓存，计数按实体而非副本计算。
export const id = 'flag-projection';
export async function run(ctx) {
  const { store, S, mkRow, checkNew } = ctx;
  const { viewEntriesCache, localFlagWritesPending, flipEntryFlag, setViewEntriesSnapshot, emptyArticlesCursor } = await import('../../dist-test/store/internals.js');
  const { awaitLocalFlagWriteFrontier, articleRowToEntry } = await import('../../dist-test/lib/api.js');
  const turn = () => new Promise((resolve) => setImmediate(resolve));
  const until = async (predicate, label) => {
    for (let count = 0; count < 150; count++) { if (predicate()) return; await turn(); }
    throw new Error(label);
  };
  let nextId = 95000;
  for (const field of ['isRead', 'isStarred']) for (const mask of [0, 1, 2, 3]) for (const evict of [false, true]) {
    ctx.useMainBackend();
    await ctx.resetStore({ settings: { ...store.getState().settings, markReadOnOpen: false, smartDedup: false } });
    const id = String(++nextId);
    const column = field === 'isRead' ? 'is_read' : 'is_starred';
    S.backendRows = [mkRow({ id: Number(id), feed_id: 10 }), mkRow({ id: Number(id) + 1000, feed_id: 12 })];
    S.feedCountsImpl = ctx.countsFromRows;
    await store.getState().reloadFromBackend();
    store.getState().selectFeed('10');
    await until(() => store.getState().backendReloadsInFlight === 0 && store.getState().entries.length === 1, 'feed A loaded');
    const copies = () => [...viewEntriesCache.values()].flatMap((snapshot) => snapshot.entries.filter((row) => row.id === id));
    checkNew(`AF1 R2 ${field} mask${mask} evict${evict} 前置：真实all/feed两份缓存`, copies().length === 2);
    const original = globalThis.__INVOKE__;
    const held = [];
    globalThis.__INVOKE__ = (cmd, args) => (field === 'isRead' ? ['mark_all_read', 'set_read'].includes(cmd) : cmd === 'set_starred')
      ? new Promise((resolve, reject) => held.push({ cmd, args, resolve, reject })) : original(cmd, args);
    try {
      if (field === 'isRead') store.getState().markCurrentViewAllRead();
      else store.getState().toggleEntryFlag(id, field);
      checkNew(`AF1 R2 ${field} mask${mask} evict${evict} 乐观投影覆盖所有副本`, copies().every((row) => row[field]));
      store.getState().toggleEntryFlag(id, field);
      store.getState().selectFeed('12');
      await until(() => store.getState().backendReloadsInFlight === 0 && store.getState().entries[0]?.feedId === '12', 'feed B loaded');
      if (evict) {
        for (let index = 0; index < 9; index++) setViewEntriesSnapshot(`article|all|${98000 + index}`, store.getState().entries, emptyArticlesCursor(1), true);
        checkNew(`AF1 R2 ${field} mask${mask} 真实8键LRU已淘汰A副本`, viewEntriesCache.size === 8 && copies().length === 0);
      }
      const countKeysAfterEviction = viewEntriesCache.size;
      for (let index = 0; index < 2; index++) {
        await until(() => held.length === index + 1, 'next same-field IPC');
        if (mask & (1 << index)) {
          S.backendRows[0][column] = index === 0;
          held[index].resolve(held[index].cmd === 'mark_all_read' ? 1 : null);
        } else held[index].reject({ message: 'precommit failure' });
        await turn();
      }
      await awaitLocalFlagWriteFrontier();
      await until(() => localFlagWritesPending() === 0, 'all actions settled');
      const expected = !!(mask & 1) && !(mask & 2);
      const expectedCount = field === 'isRead' ? Number(!expected) : Number(expected);
      checkNew(`AF1 R2 ${field} mask${mask} evict${evict} 离屏结算：字段/计数一次收敛`,
        copies().every((row) => row[field] === expected)
        && (evict || copies().length === 2)
        && store.getState().feedCounts.get('10')?.[field === 'isRead' ? 'unread' : 'starred'] === expectedCount
        && store.getState().entries[0]?.feedId === '12' && !store.getState().entries[0]?.[field]);
      if (evict) checkNew(`AF1 R2 ${field} mask${mask} 淘汰缓存不重建`, viewEntriesCache.size === countKeysAfterEviction);
      else {
        store.getState().selectFeed('10');
        checkNew(`AF1 R2 ${field} mask${mask} 回A同步第一态正确（不等reload）`, store.getState().entries[0]?.[field] === expected);
        await until(() => store.getState().backendReloadsInFlight === 0, 'return A reload complete');
      }
    } finally { globalThis.__INVOKE__ = original; }
  }
  for (const mode of ['same-field-takeover', 'other-field-write', 'source-moved-offscreen', 'conflicting-cache-sources', 'deleted-and-evicted']) {
    ctx.useMainBackend();
    await ctx.resetStore({ settings: { ...store.getState().settings, markReadOnOpen: false, smartDedup: false } });
    const id = String(++nextId);
    S.backendRows = [mkRow({ id: Number(id), feed_id: 10 }), mkRow({ id: Number(id) + 1000, feed_id: 12 })];
    S.feedCountsImpl = ctx.countsFromRows;
    await store.getState().reloadFromBackend();
    store.getState().selectFeed('10');
    await until(() => store.getState().backendReloadsInFlight === 0, 'feed A');
    const original = globalThis.__INVOKE__;
    const held = [];
    const heldCounts = [];
    let holdCounts = false;
    globalThis.__INVOKE__ = (cmd, args) => ['mark_all_read', 'set_read'].includes(cmd)
      ? new Promise((resolve, reject) => held.push({ cmd, args, resolve, reject }))
      : cmd === 'counts_snapshot' && holdCounts ? new Promise((resolve) => heldCounts.push(resolve)) : original(cmd, args);
    try {
      store.getState().markCurrentViewAllRead(); store.getState().toggleEntryFlag(id, 'isRead');
      if (mode === 'same-field-takeover') flipEntryFlag(id, 'isRead');
      if (mode === 'other-field-write') { flipEntryFlag(id, 'isStarred'); S.backendRows[0].is_starred = true; }
      store.getState().selectFeed('12');
      await until(() => store.getState().backendReloadsInFlight === 0 && store.getState().entries[0]?.feedId === '12', 'feed B');
      if (mode === 'source-moved-offscreen') S.backendRows[0].feed_id = 11;
      if (mode === 'conflicting-cache-sources') {
        S.backendRows[0].feed_id = 11;
        // 两份曾接受过的视图快照带不同feed：不能按扫描到的第一份旧来源计数。
        setViewEntriesSnapshot('article|all|all', S.backendRows.map(articleRowToEntry), emptyArticlesCursor(2), true);
      }
      if (mode === 'deleted-and-evicted') { S.backendRows.shift(); viewEntriesCache.clear(); }
      await until(() => held.length === 1, 'first IPC');
      if (mode !== 'deleted-and-evicted') S.backendRows[0].is_read = true;
      held[0].resolve(1);
      await until(() => held.length === 2, 'second IPC');
      await turn(); await turn();
      holdCounts = mode === 'conflicting-cache-sources';
      held[1].reject({ message: 'old pending unread rejected' });
      await awaitLocalFlagWriteFrontier();
      await until(() => localFlagWritesPending() === 0, 'settled');
      if (holdCounts) {
        await until(() => heldCounts.length > 0, 'source uncertainty reads authoritative counts');
        checkNew('AF1 R2 多cache脏来源：权威读取完成前不把差值归给旧feed10', store.getState().feedCounts.get('10')?.unread === 1);
        holdCounts = false;
        for (const resolve of heldCounts) resolve({ feeds: ctx.countsFromRows(), scope: null });
      }
      if (mode === 'source-moved-offscreen' || mode === 'conflicting-cache-sources' || mode === 'deleted-and-evicted') {
        await until(() => !store.getState().feedCounts.has('10'), 'bounded authoritative counts after uncertain source/deletion');
      }
      const copies = [...viewEntriesCache.values()].flatMap((snapshot) => snapshot.entries.filter((row) => row.id === id));
      checkNew(`AF1 R2 ${mode}：旧结算不覆盖接管字段、不按旧源计数/重建缓存`,
        (mode === 'deleted-and-evicted' ? copies.length === 0 && viewEntriesCache.size === 0 : copies.length === 2 && copies.every((row) => row.isRead))
        && store.getState().entries.every((row) => row.feedId === '12' && !row.isRead)
        && (mode === 'source-moved-offscreen' || mode === 'conflicting-cache-sources' ? store.getState().feedCounts.get('11')?.unread === 0
          : mode === 'deleted-and-evicted' || store.getState().feedCounts.get('10')?.unread === 0));
      if (mode === 'other-field-write') checkNew('AF1 R2 另一字段不接管读态且所有副本/计数保留收藏',
        copies.every((row) => row.isStarred) && store.getState().feedCounts.get('10')?.starred === 1);
    } finally { globalThis.__INVOKE__ = original; }
  }
  // 非线性聚合只接受后端scopeCounts，不由字段副本数推算。
  ctx.useMainBackend();
  await ctx.resetStore({ settings: { ...store.getState().settings, markReadOnOpen: false, smartDedup: true } });
  S.backendRows = [mkRow({ id: 96001, feed_id: 10 }), mkRow({ id: 96002, feed_id: 10 })];
  S.feedCountsImpl = ctx.countsFromRows;
  S.scopeCountsImpl = () => ({ all: { total: 1, unread: Number(S.backendRows.some((row) => !row.is_read)), starred: 0, today: 0 }, folders: [] });
  await store.getState().reloadFromBackend();
  const beforeScope = store.getState().scopeCounts;
  store.getState().markEntriesReadBulk(['96001', '96002']);
  checkNew('AF1 R2 scope非线性：两条乐观读写不对scopeCounts线性减2', store.getState().scopeCounts === beforeScope);
  await awaitLocalFlagWriteFrontier();
  await until(() => store.getState().scopeCounts?.all.unread === 0, 'backend non-linear scope reconciliation');
  checkNew('AF1 R2 scope非线性：最终源unread0、聚合total仍1', store.getState().feedCounts.get('10')?.unread === 0 && store.getState().scopeCounts?.all.total === 1);
}
