// 外部 IPC 的时机/结果受控；驱动生产 store，不替换产品回滚或缓存逻辑。
export const id = 'audit-repairs';
export async function run(ctx) {
  const { S, store, mkRow, resetStore, checkNew, nTick } = ctx;
  ctx.useMainBackend();
  // 旧smartDedup开/关已删除；保留整套结果矩阵并改测权威scope快照与缺scope回退两条实际读取路径。
  const withScopeCounts = (enabled) => {
    if (!enabled) return;
    S.scopeCountsImpl = () => {
      const own = ctx.countsFromRows().find((row) => row.feed_id === 10);
      const counts = { total: own.total, unread: own.unread, starred: own.starred, today: own.today };
      return { all: counts, folders: [{scope_id:1,...counts}], feeds: [{scope_id:10,...counts}] };
    };
  };
  for (const field of ['isRead', 'isStarred']) for (const initial of [false, true]) for (const scopeSnapshot of [false, true]) {
    for (const count of [2, 3]) {
      for (let mask = 0; mask < (1 << count); mask++) {
        await resetStore({ settings: { ...store.getState().settings, markReadOnOpen: false } });
        const column = field === 'isRead' ? 'is_read' : 'is_starred';
        S.backendRows = [mkRow({ id: 88001, [column]: initial })];
        S.feedCountsImpl = ctx.countsFromRows;
        withScopeCounts(scopeSnapshot);
        await store.getState().reloadFromBackend();
        const original = globalThis.__INVOKE__;
        const held = [];
        const command = field === 'isRead' ? 'set_read' : 'set_starred';
        globalThis.__INVOKE__ = (cmd, args) => cmd === command
          ? new Promise((resolve, reject) => held.push({ args, resolve, reject }))
          : original(cmd, args);
        try {
          for (let index = 0; index < count; index++) store.getState().toggleEntryFlag('88001', field);
          await nTick(2);
          let serial = true;
          let expected = initial;
          for (let index = 0; index < count; index++) {
            serial &&= held.length === index + 1;
            const request = held[index];
            if (mask & (1 << index)) {
              expected = index % 2 === 0 ? !initial : initial;
              S.backendRows[0][column] = expected;
              request.resolve(field === 'isRead' ? [{id:88001,feed_id:10}] : null);
            } else request.reject({ message: 'transaction rejected before commit' });
            await nTick(4);
          }
          await nTick(35);
          checkNew(`AF1 ${field} initial=${initial} scopeSnapshot=${scopeSnapshot} ${count}意图mask=${mask}：实际串行提交、末成功意图与计数`,
            serial && held.length === count && store.getState().entries[0][field] === expected && S.backendRows[0][column] === expected
            && store.getState().feedCounts.get('10')?.[field === 'isRead' ? 'unread' : 'starred'] === (field === 'isRead' ? Number(!expected) : Number(expected))
            && ctx.selectViewCounts(store.getState())[field === 'isRead' ? 'unread' : 'starred'] === (field === 'isRead' ? Number(!expected) : Number(expected)));
        } finally { globalThis.__INVOKE__ = original; }
      }
    }
  }
  for (const snippetChanged of [false, true]) {
    await resetStore({ settings: { ...store.getState().settings, markReadOnOpen: false, defaultOpenMode: 'rss' } });
    const first = mkRow({ id: 88002, content_id: 41, content_revision: 1, snippet: 'old snippet', content_html: '<p>old body</p>' });
    S.backendRows = [first];
    S.getArticlesPlan = { rows: [first] };
    await store.getState().reloadFromBackend();
    store.getState().hydrateArticleContent(['88002']);
    await nTick(3);
    let release;
    const original = globalThis.__INVOKE__;
    globalThis.__INVOKE__ = (cmd, args) => cmd === 'get_article' ? new Promise((resolve) => { release = resolve; }) : original(cmd, args);
    try {
      store.getState().selectArticle('88002');
      await nTick(3);
      S.backendRows = [{ ...first, content_id: 42, content_revision: 1, snippet: snippetChanged ? 'new snippet' : first.snippet, content_html: '<p>new body</p>' }];
      S.getArticlesPlan = { rows: S.backendRows };
      await store.getState().reloadFromBackend();
      store.getState().hydrateArticleContent(['88002']);
      await nTick(3);
      release(first);
      await nTick(3);
      checkNew(`NF1 新content_id且snippet变化=${snippetChanged}：迟到详情不覆盖正文/轻字段`,
        ctx.getBodyEntry('88002')?.content === '<p>new body</p>' && ctx.getBodyEntry('88002')?.contentId === 42
        && store.getState().entries[0].snippet === S.backendRows[0].snippet);
    } finally { globalThis.__INVOKE__ = original; }
  }
  for (const field of ['isRead', 'isStarred']) {
    await resetStore({ settings: { ...store.getState().settings } });
    S.backendRows = [mkRow({ id: 88003, is_read: false, is_starred: false })];
    S.feedCountsImpl = ctx.countsFromRows;
    await store.getState().reloadFromBackend();
    const held = [];
    const original = globalThis.__INVOKE__;
    globalThis.__INVOKE__ = (cmd, args) => cmd === (field === 'isRead' ? 'set_read' : 'set_starred')
      ? new Promise((resolve, reject) => held.push({ args, resolve, reject })) : original(cmd, args);
    try {
      store.getState().toggleEntryFlag('88003', field);
      store.getState().toggleEntryFlag('88003', field);
      await nTick(2);
      held[0].reject({ message: 'first rejected' });
      await nTick(3);
      // 首笔失败后第二笔已发送、第三笔点击保持同一意图链但不能越过第二笔。
      store.getState().toggleEntryFlag('88003', field);
      await nTick(2);
      checkNew(`AF1 ${field} 第三次点击仍即时乐观、仅两笔已发出`, held.length === 2 && store.getState().entries[0][field] === true);
      held[1].reject({ message: 'second rejected' });
      await nTick(3);
      held[2].reject({ message: 'third rejected' });
      await nTick(35);
      checkNew(`F1 ${field} settle间插入第三次toggle：三失败恢复基线`, store.getState().entries[0][field] === false);
      store.getState().toggleEntryFlag('88003', field);
      await nTick(2);
      S.backendRows[0][field === 'isRead' ? 'is_read' : 'is_starred'] = true;
      held[3].resolve(null);
      await nTick(35);
      checkNew(`F1 ${field} 失败链结束后新toggle：新版本正常成功`, store.getState().entries[0][field] === true);
    } finally { globalThis.__INVOKE__ = original; }
  }
  // 各标读入口必须与toggle共享实际IPC顺序；成功不能被旧请求逆序提交覆盖。
  for (const scopeSnapshot of [false, true]) for (const mode of ['bulk-first', 'open-first', 'anchor-first', 'all-first', 'toggle-first-bulk', 'toggle-first-open', 'toggle-first-all']) {
    for (const mask of [0, 1, 2, 3]) {
      const initial = mode.startsWith('toggle-first');
      await resetStore({ settings: { ...store.getState().settings, markReadOnOpen: true, defaultOpenMode: 'rss' } });
      S.backendRows = [mkRow({ id: 88004, is_read: initial })];
      S.feedCountsImpl = ctx.countsFromRows;
      withScopeCounts(scopeSnapshot);
      S.detailImpl = () => S.backendRows[0];
      await store.getState().reloadFromBackend();
      const original = globalThis.__INVOKE__;
      const held = [];
      globalThis.__INVOKE__ = (cmd, args) => ['set_read', 'set_read_bulk', 'mark_all_read'].includes(cmd)
        ? new Promise((resolve, reject) => held.push({ cmd, args, resolve, reject })) : original(cmd, args);
      const read = async () => {
        if (mode.includes('bulk')) store.getState().markEntriesReadBulk(['88004']);
        else if (mode.includes('anchor')) await store.getState().anchorToArticle('88004');
        else if (mode.includes('open')) store.getState().selectArticle('88004');
        else store.getState().markCurrentViewAllRead();
      };
      try {
        if (initial) { store.getState().toggleEntryFlag('88004', 'isRead'); await read(); }
        else { await read(); store.getState().toggleEntryFlag('88004', 'isRead'); }
        await nTick(2);
        let serial = true;
        let expected = initial;
        for (let index = 0; index < 2; index++) {
          serial &&= held.length === index + 1;
          const request = held[index];
          if (mask & (1 << index)) {
            expected = request.cmd === 'set_read' ? request.args.read : true;
            S.backendRows[0].is_read = expected;
            request.resolve(request.cmd === 'mark_all_read' ? 1 : [{id:88004,feed_id:10}]);
          } else request.reject({ message: 'precommit rejected' });
          await nTick(5);
        }
        await nTick(35);
        checkNew(`AF1 ${mode} scopeSnapshot=${scopeSnapshot} mask=${mask}：共同执行前沿、末成功意图/失败基线/计数`, serial
          && store.getState().entries[0].isRead === expected
          && store.getState().feedCounts.get('10')?.unread === Number(!expected)
          && ctx.selectViewCounts(store.getState()).unread === Number(!expected));
      } finally { globalThis.__INVOKE__ = original; }
    }
  }
  {
    await resetStore({ settings: { ...store.getState().settings, markReadOnOpen: false } });
    S.backendRows = [mkRow({ id: 88005 })]; S.feedCountsImpl = ctx.countsFromRows;
    await store.getState().reloadFromBackend();
    const original = globalThis.__INVOKE__;
    const held = [];
    globalThis.__INVOKE__ = (cmd, args) => ['set_read', 'set_starred'].includes(cmd)
      ? new Promise((resolve) => held.push({ cmd, args, resolve })) : original(cmd, args);
    try {
      store.getState().toggleEntryFlag('88005', 'isRead');
      store.getState().toggleEntryFlag('88005', 'isStarred');
      await nTick(2);
      checkNew('AF1 read/star前沿独立：两字段均实际发出且保持即时乐观', held.length === 2
        && store.getState().entries[0].isRead && store.getState().entries[0].isStarred);
      S.backendRows[0].is_starred = true;
      held.find(({ cmd }) => cmd === 'set_starred').resolve(null);
      await nTick(2);
      S.backendRows[0].is_read = true;
      held.find(({ cmd }) => cmd === 'set_read').resolve(null);
      await nTick(5);
    } finally { globalThis.__INVOKE__ = original; }
  }
  for (const change of ['switch-view', 'move-source', 'delete-entry']) {
    await resetStore({ settings: { ...store.getState().settings, markReadOnOpen: false } });
    S.backendRows = [mkRow({ id: 88006 })]; S.feedCountsImpl = ctx.countsFromRows;
    await store.getState().reloadFromBackend();
    const original = globalThis.__INVOKE__;
    const held = [];
    globalThis.__INVOKE__ = (cmd, args) => cmd === 'set_read'
      ? new Promise((resolve, reject) => held.push({ args, resolve, reject })) : original(cmd, args);
    try {
      store.getState().toggleEntryFlag('88006', 'isRead'); store.getState().toggleEntryFlag('88006', 'isRead');
      await nTick(2);
      if (change === 'switch-view') store.getState().selectFeed('12');
      else if (change === 'move-source') { S.backendRows[0].feed_id = 11; store.getState().selectLayout('social'); }
      else { S.backendRows = []; await store.getState().reloadFromBackend(); }
      await nTick(4);
      held[0].reject({ message: 'old first rejected' }); await nTick(3);
      held[1].reject({ message: 'old second rejected' }); await nTick(35);
      const { localFlagWritesPending } = await import('../../dist-test/store/internals.js');
      const row = store.getState().entries.find((entry) => entry.id === '88006');
      checkNew(`AF1 ${change}在途旧写结算：不复活窗口外实体，明确删除只保不可用占位并释放在途`, localFlagWritesPending() === 0
        && (change === 'move-source' ? row?.feedId === '11' && !row.isRead
          : change === 'delete-entry' ? row?.unavailable === true && row?.bodyMissing === true : !row));
      if (change === 'move-source') checkNew('AF1 来源迁移后计数跟随当前来源且无旧乐观残留',
        store.getState().feedCounts.get('11')?.unread === 1);
      // 旧链释放后同id的新操作仍能成功，不能留下锁住字段的队尾/声明。
      S.backendRows = [mkRow({ id: 88006 })];
      store.setState({ activeFeedFilter: 'all', activeContentLayout: 'article' });
      await store.getState().reloadFromBackend();
      store.getState().toggleEntryFlag('88006', 'isRead'); await nTick(2);
      S.backendRows[0].is_read = true; held[2].resolve(null); await nTick(5);
      checkNew(`AF1 ${change}后新操作不继承旧链`, store.getState().entries[0].isRead && localFlagWritesPending() === 0);
    } finally { globalThis.__INVOKE__ = original; }
  }
  {
    const { markBodyLoading, applyBodyRow, dropBodyEntry, getBodyEntry, BODY_CACHE_MAX, resetBodyCacheForTests } = await import('../../dist-test/store/bodyCache.js');
    resetBodyCacheForTests();
    const stale = markBodyLoading('evicted');
    for (let index = 0; index < BODY_CACHE_MAX; index++) markBodyLoading(`budget-${index}`);
    checkNew('NF1 LRU淘汰在途记录：迟到旧响应不能重建被淘汰正文',
      !applyBodyRow('evicted', { content_html: '<p>old</p>' }, stale, { aiSummary: '', translatedContent: '' }) && !getBodyEntry('evicted'));
    const fresh = markBodyLoading('evicted');
    applyBodyRow('evicted', { content_html: '<p>new</p>' }, fresh, { aiSummary: '', translatedContent: '' });
    checkNew('NF1 LRU重入队：新戳不复用且拒绝旧响应', fresh !== stale
      && !applyBodyRow('evicted', { content_html: '<p>old</p>' }, stale, { aiSummary: '', translatedContent: '' })
      && getBodyEntry('evicted')?.content === '<p>new</p>');
    dropBodyEntry('evicted');
    const retry = markBodyLoading('evicted');
    checkNew('NF1 显式重试删档：新戳同样拒绝之前在途响应', retry !== fresh
      && !applyBodyRow('evicted', { content_html: '<p>old</p>' }, fresh, { aiSummary: '', translatedContent: '' }));
    resetBodyCacheForTests();
  }
}
