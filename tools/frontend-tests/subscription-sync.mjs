// tools/frontend-tests/subscription-sync.mjs
// 领域模块：订阅添加后的回读/同步尾链（OPT-007-UI 补件）
//
// 缺陷：addFeed 在本地添加成功 → reload 之后，若勾选同步且已连接会执行
// `await api.syncLocalFeeds().catch(() => null)`——该操作会导入/回填订阅，
// 之后没有回读：导入的订阅、回填的标题在本地不可见；同步失败被吞进「已添加」
// 成功路径；而一旦后续（同步/刷新）失败又整体落入「添加失败」，重试会再次
// 新增。
//
// 修后契约（本域逐条锁定）：
// - 无同步选择 / 未连接：保持原有单次回读，不发 syncLocalFeeds、不额外请求；
// - 勾选同步且已连接：syncLocalFeeds 之后（整体成功与部分失败两个分支）都
//   再回读一次本地真实结果——已写入的订阅（含失败前已导入的部分）可见；
// - 同步成功不谎称：成功才「已添加订阅源：…」；同步/刷新失败如实说
//   「订阅已添加，同步/刷新失败」；失败提示带重试入口，且重试只重跑本段，
//   绝不重新 addFeed（不再次新增）；
// - 添加本身（add_feed IPC）失败维持既有「添加失败」+ 重试=重新添加。
//
// 取证方式：真实 store + 受控 IPC 假后端（独立读写数据 B：add_feed 真写入、
// sync_local_feeds 真改数据、list_folders/list_feeds 真读取），不用源码文本
// 断言代替行为；临时 __INVOKE__ 覆盖经 finally 恢复。
export const id = 'subscription-sync';

export async function run(ctx) {
  const { S, store, checkNew, nTick, bootFixture, overrideGlobal, FOLDERS, FEEDS } = ctx;
  await ctx.useMainBackend();

  const deepCopy = (x) => JSON.parse(JSON.stringify(x));
  /* 独立后端真实数据（读写都发生在这里；与共享夹具解耦，逐用例复位） */
  const B = { folders: [], feeds: [], lastAddedId: null, addCalls: 0 };
  const be = {
    connected: true,          // sync_status 的 connected
    syncPlan: 'ok',           // 'ok' | 'reject-partial'（失败前已导入）| 'reject-clean'
    failAdd: false,           // add_feed 注入失败
    rejectNextReload: false,  // 下一次 reload 的 folders/feeds 拉取失败（仅一次）
  };
  const remoteRow = (id, name) => ({
    id, folder_id: 1, feed_url: `https://remote.example/${id}.rss`, site_url: null,
    title: name, favicon_url: null, layout: 'inherit', auto_summary: false, auto_translate: false,
    fetch_failed: false, fetch_error: null, last_fetched_at: null,
  });
  const prevInvoke = globalThis.__INVOKE__;
  const restoreInvoke = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
    S.invokeCalls.push({ cmd, args });
    switch (cmd) {
      case 'list_folders': {
        if (be.rejectNextReload) {
          be.rejectNextReload = false; // 只拒一次（重试路径必须能恢复）
          return Promise.reject({ message: '注入：首次刷新失败' });
        }
        return Promise.resolve(deepCopy(B.folders));
      }
      case 'list_feeds':
        return Promise.resolve(deepCopy(B.feeds));
      case 'add_feed': {
        B.addCalls += 1;
        if (be.failAdd) return Promise.reject({ message: '注入：添加失败' });
        const id = 90 + B.addCalls;
        B.lastAddedId = id;
        B.feeds.push({
          id, folder_id: Number(args.folderId) || 1, feed_url: args.feedUrl, site_url: null,
          title: args.title || args.feedUrl, favicon_url: null, layout: 'inherit',
          auto_summary: false, auto_translate: false, fetch_failed: false, fetch_error: null, last_fetched_at: null,
        });
        return Promise.resolve(deepCopy(B.feeds.find((f) => f.id === id)));
      }
      case 'sync_local_feeds': {
        if (be.syncPlan === 'reject-partial') {
          /* 部分失败：失败前同步已导入一条远端订阅（本地已写入——必须仍可见） */
          B.feeds.push(remoteRow(501, '远端部分导入源'));
          return Promise.reject({ message: '注入：同步失败' });
        }
        if (be.syncPlan === 'reject-clean') {
          return Promise.reject({ message: '注入：同步失败' });
        }
        /* 成功：回填本地新源的标题（远端身份/标题回填）+ 导入另一条远端订阅 */
        const added = B.feeds.find((f) => f.id === B.lastAddedId);
        if (added) added.title = '远端回填标题';
        B.feeds.push(remoteRow(502, '远端导入源'));
        return Promise.resolve('已同步 2 个订阅');
      }
      case 'sync_status':
        return Promise.resolve({ connected: be.connected });
      default:
        return prevInvoke(cmd, args);
    }
  });

  /* 逐用例复位：后端数据回基线 + 夹具重装 + 清账 */
  const resetCase = async () => {
    B.folders = deepCopy(FOLDERS);
    B.feeds = deepCopy(FEEDS);
    B.lastAddedId = null;
    B.addCalls = 0;
    be.connected = true;
    be.syncPlan = 'ok';
    be.failAdd = false;
    be.rejectNextReload = false;
    await bootFixture();
    store.setState({ syncConnected: true, syncStatus: 'synced', toasts: [] });
    S.invokeCalls.length = 0;
  };
  const countCmd = (cmd) => S.invokeCalls.filter((c) => c.cmd === cmd).length;
  const toastsText = () => store.getState().toasts.map((t) => t.text);
  const feedUrls = () => store.getState().categories.flatMap((c) => c.feeds.map((f) => f.url));
  const feedNames = () => store.getState().categories.flatMap((c) => c.feeds.map((f) => f.name));
  const indexNames = () => [...store.getState().feedIndex.values()].map(({ feed }) => feed.name);

  try {
    /* ============================================================
       A：勾选同步且已连接——syncLocalFeeds 成功后必须回读真实结果
       ============================================================ */
    await resetCase();
    store.getState().addFeed('cat-1', 'https://new.example/rss', '新源', 'inherit', 'inherit', 'inherit', true);
    await nTick(40);
    checkNew('(opt007ui) 同步成功：syncLocalFeeds 恰好一次', countCmd('sync_local_feeds') === 1);
    checkNew('(opt007ui) 同步成功：同步段确实再回读一次（list_folders 恰两次：首次可见 + 同步后对账）',
      countCmd('list_folders') === 2);
    checkNew('(opt007ui) 同步成功：远端回填的本源标题在 categories/feedIndex 真实可见（回读落地，非假后端自证）',
      indexNames().includes('远端回填标题') && feedNames().includes('远端回填标题')
      && feedUrls().includes('https://new.example/rss'));
    checkNew('(opt007ui) 同步成功：同步导入的另一条订阅在 categories/feedIndex 真实可见',
      indexNames().includes('远端导入源') && feedNames().includes('远端导入源'));
    checkNew('(opt007ui) 同步成功：成功提示与状态合理（已添加订阅源 / synced）',
      toastsText().includes('已添加订阅源：新源') && store.getState().syncStatus === 'synced');

    /* ============================================================
       B：无同步选择——不额外回读、不发 syncLocalFeeds
       ============================================================ */
    await resetCase();
    store.getState().addFeed('cat-1', 'https://new2.example/rss', '无同步源', 'inherit', 'inherit', 'inherit', false);
    await nTick(40);
    checkNew('(opt007ui) 无同步选择：不发 syncLocalFeeds', countCmd('sync_local_feeds') === 0);
    checkNew('(opt007ui) 无同步选择：不额外回读（list_folders 恰一次，既有单次回读保留）',
      countCmd('list_folders') === 1);
    checkNew('(opt007ui) 无同步选择：既有成功路径逐字保留（已添加订阅源 / synced）',
      toastsText().includes('已添加订阅源：无同步源') && store.getState().syncStatus === 'synced');

    /* ============================================================
       B2：勾选同步但后端未连接——同样不额外回读
       ============================================================ */
    await resetCase();
    be.connected = false;
    store.setState({ syncConnected: false });
    store.getState().addFeed('cat-1', 'https://new3.example/rss', '未连接源', 'inherit', 'inherit', 'inherit', true);
    await nTick(40);
    checkNew('(opt007ui) 未连接（勾选同步）：不发 syncLocalFeeds、不额外回读', countCmd('sync_local_feeds') === 0 && countCmd('list_folders') === 1);
    checkNew('(opt007ui) 未连接：新源仍可见、成功提示照常',
      feedUrls().includes('https://new3.example/rss') && toastsText().includes('已添加订阅源：未连接源'));

    /* ============================================================
       C：同步部分失败——失败前已导入的订阅仍回读可见；不谎称完整成功；
          重试只重跑本段（绝不再次新增）
       ============================================================ */
    await resetCase();
    be.syncPlan = 'reject-partial';
    store.getState().addFeed('cat-1', 'https://new4.example/rss', '同步败源', 'inherit', 'inherit', 'inherit', true);
    await nTick(40);
    checkNew('(opt007ui) 同步失败：同步后回读照常执行（list_folders 两次）——失败分支不得跳过回读',
      countCmd('list_folders') === 2);
    checkNew('(opt007ui) 同步失败但失败前已导入的订阅真实可见（部分改变不回滚、不静默不可见）',
      indexNames().includes('远端部分导入源') && feedNames().includes('远端部分导入源'));
    checkNew('(opt007ui) 同步失败：不谎称完整成功（无成功文案；提示如实「订阅已添加，同步失败」）',
      !toastsText().includes('已添加订阅源：同步败源')
      && toastsText().includes('订阅已添加，同步失败：注入：同步失败'));
    checkNew('(opt007ui) 同步失败：syncStatus=error，重试入口保留',
      store.getState().syncStatus === 'error' && store.getState().toasts.at(-1)?.action?.label === '重试');
    /* 重试：只重跑回读/同步段——绝不重新 addFeed（那会再新增一条） */
    be.syncPlan = 'ok';
    S.invokeCalls.length = 0;
    store.getState().toasts.at(-1)?.action?.run?.();
    await nTick(40);
    checkNew('(opt007ui) 同步失败重试：不重新 addFeed（add_feed 0 次），syncLocalFeeds 重试成功并回读',
      countCmd('add_feed') === 0 && countCmd('sync_local_feeds') === 1 && countCmd('list_folders') === 2);
    checkNew('(opt007ui) 同步失败重试成功后：提示与状态回到如实成功',
      toastsText().includes('已添加订阅源：同步败源') && store.getState().syncStatus === 'synced');

    /* ============================================================
       D：首次刷新失败——不伪装成「添加失败」（重试不再新增）；
          重试重跑本段（刷新 → 同步 → 回读）
       ============================================================ */
    await resetCase();
    be.rejectNextReload = true;
    store.getState().addFeed('cat-1', 'https://new5.example/rss', '刷新败源', 'inherit', 'inherit', 'inherit', true);
    await nTick(40);
    checkNew('(opt007ui) 首次刷新失败：提示如实「订阅已添加，刷新失败」，不出现「添加失败」',
      !toastsText().some((t) => t.startsWith('添加失败'))
      && toastsText().some((t) => t.startsWith('订阅已添加，刷新失败')));
    checkNew('(opt007ui) 首次刷新失败：syncStatus=error、带重试入口；本段未继续跑 sync（如实不越段）',
      store.getState().syncStatus === 'error' && store.getState().toasts.at(-1)?.action?.label === '重试'
      && countCmd('sync_local_feeds') === 0);
    checkNew('(opt007ui) 首次刷新失败：新增订阅此刻不可见（如实——失败即失败），但可由重试恢复',
      !feedUrls().includes('https://new5.example/rss'));
    S.invokeCalls.length = 0;
    store.getState().toasts.at(-1)?.action?.run?.();
    await nTick(40);
    checkNew('(opt007ui) 刷新失败重试：不重新 addFeed，刷新恢复后新源可见并续跑同步成功',
      countCmd('add_feed') === 0 && feedUrls().includes('https://new5.example/rss')
      && countCmd('sync_local_feeds') === 1 && store.getState().syncStatus === 'synced');

    /* ============================================================
       E：添加本身失败（add_feed IPC reject）——维持既有「添加失败」+
          重试=重新添加（该次添加根本没有发生）
       ============================================================ */
    await resetCase();
    be.failAdd = true;
    store.getState().addFeed('cat-1', 'https://new6.example/rss', '添加败源', 'inherit', 'inherit', 'inherit', true);
    await nTick(40);
    checkNew('(opt007ui) 添加本身失败：维持既有「添加失败」文案 + 重试入口 + error 状态',
      toastsText().some((t) => t === '添加失败：注入：添加失败')
      && store.getState().toasts.at(-1)?.action?.label === '重试'
      && store.getState().syncStatus === 'error');
    S.invokeCalls.length = 0;
    be.failAdd = false;
    store.getState().toasts.at(-1)?.action?.run?.();
    await nTick(40);
    checkNew('(opt007ui) 添加失败重试：确为重新添加（add_feed 再次发出）且成功后路径照常',
      countCmd('add_feed') === 1 && toastsText().includes('已添加订阅源：添加败源'));
  } finally {
    restoreInvoke();
    await ctx.resetStore();
  }
}
