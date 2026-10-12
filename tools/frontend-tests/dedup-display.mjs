// tools/frontend-tests/dedup-display.mjs
// 领域模块：OPT-008B 第一阶段 R1——smartDedup 显示聚合的 store 接线（计数来源、
// 有序对账、迟到响应守卫、设置持久化顺序、搜索结果打开）。
//
// 与首版（静态 DETAIL 自证）不同：本域用**独立持久态假后端**（真实行数组 +
// 与 Rust 同构的显示聚合：scope × 视图内按 url 组选最小 id 代表；read 本地
// 闭包与 markAll 闭包也如实模拟）——断言数字来自真实重算，而非静态表。
//
// 契约：
// - feed_counts = 布局无关的按源（源内 scope）计数；scope_counts 单独入
//   scopeCounts 快照（带 layout/dedup 元数据），选择器仅在布局匹配时消费；
// - 单条读/藏、批量、回滚之后按**真实计数对账**（折叠组不做线性 ±1 推算）；
// - 迟到 scope_counts 不得跨布局回填（布局/开关/计数落地序号守卫）；
// - smartDedup 更新必须等待落库成功再 reload（写失败回滚到已确认值 + toast）；
// - 搜索结果行即使不在当前列表 scope 的显示集合里也必须可真实打开。
export const id = 'dedup-display';

/** R5 回归用：受控 deferred（本文件不依赖组件夹具的 deferred 导出）。 */
const mkDeferred = () => {
  let resolve;
  const promise = new Promise((r) => { resolve = r; });
  return { promise, resolve };
};

export async function run(ctx) {
  const { S, store, checkNew, nTick, resetStore, overrideGlobal, selectViewCounts, selectTreeCounts } = ctx;
  await ctx.useMainBackend();

  const NOW = Date.now();
  const OLD = new Date(NOW - 3 * 86400000).toISOString();
  const localDayKey = (ms) => { const d = new Date(ms); return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`; };

  /* ---------- 持久态假后端 ---------- */
  const FOLDERS = [
    { id: 1, name: '技术', layout: 'article', auto_summary: false, auto_translate: false, collapsed: false },
    { id: 2, name: '社交', layout: 'social', auto_summary: false, auto_translate: false, collapsed: false },
  ];
  const FEEDS = [
    { id: 10, folder_id: 1, feed_url: 'https://a.example/rss', site_url: null, title: '源A', favicon_url: null, layout: 'inherit', auto_summary: false, auto_translate: false, fetch_failed: false, fetch_error: null, last_fetched_at: null },
    { id: 12, folder_id: 1, feed_url: 'https://d.example/rss', site_url: null, title: '源D', favicon_url: null, layout: 'inherit', auto_summary: false, auto_translate: false, fetch_failed: false, fetch_error: null, last_fetched_at: null },
    { id: 20, folder_id: 2, feed_url: 'https://c.example/rss', site_url: null, title: '源C', favicon_url: null, layout: 'inherit', auto_summary: false, auto_translate: false, fetch_failed: false, fetch_error: null, last_fetched_at: null },
  ];
  const mkRow = (o) => ({
    id: 0, feed_id: 10, title: 't', author: null, snippet: 's', image_url: null,
    enclosure_url: null, enclosure_mime: null, duration_sec: null, ai_summary: null,
    source: 'direct', published_at: OLD, is_read: false, is_starred: false, url: null,
    content_html: null, translated_content: null, fulltext_extracted: false, ...o,
  });

  const B = {
    rows: [],
    settingsRaw: null,      // app_settings 原文（后端读开关）
    scopeCalls: [],         // 每次 counts_snapshot 的 layout（调用序）
    deferCountsOnce: false, // true → 下一次 counts_snapshot 挂起（载荷=发起时刻）
    pendingCounts: [],
    deferReadId: null,      // 指定 id 的下一次 set_read(true) 挂起
    pendingRead: [],
    deferStarId: null,      // 指定 id 的下一次 set_starred(true) 挂起（R2：读/藏多请求在途）
    pendingStar: [],
    deferGetSettingOnce: null, // R3：下一次 get_setting 挂起并返回该原文（旧读探针）
    pendingGetSetting: [],
    rejectGetSettingOnce: false, // R4：下一次 get_setting 直接拒绝（回读失败/校准失败探针）
    rejectGetSettingCount: 0,    // R6：接下来 N 次 get_setting 直接拒绝（连读失败探针）
    getSettingCalls: 0,
    setSettingCalls: 0,
    setSettingPlan: null,   // null | { mode: 'defer' } | { mode: 'reject' }
    setSettingPlans: [],    // 顺序计划队列（先出队者先消费；耗尽后看 setSettingPlan）
    pendingSetSetting: [],
    failNextCounts: false,
    listCalls: 0,
    getArticleCalls: 0,
  };
  const feedLayout = (fid) => {
    const f = FEEDS.find((x) => x.id === fid);
    const fo = FOLDERS.find((x) => x.id === f.folder_id);
    return f.layout !== 'inherit' ? f.layout : fo.layout;
  };
  const backendDedup = () => !!JSON.parse(B.settingsRaw ?? '{}').smartDedup;
  const inArgsScope = (r, a) => {
    if (a.feed_id != null && r.feed_id !== a.feed_id) return false;
    if (a.folder_id != null && (FEEDS.find((f) => f.id === r.feed_id) || {}).folder_id !== a.folder_id) return false;
    if (a.layout && feedLayout(r.feed_id) !== a.layout) return false;
    if (a.only_unread && r.is_read) return false;
    if (a.only_starred && !r.is_starred) return false;
    if (a.only_today && localDayKey(Date.parse(r.published_at)) !== localDayKey(NOW)) return false;
    return true;
  };
  /* 显示聚合：同 url 组在**匹配集合内**选最小 id 代表（与 Rust 谓词同构）；
     url 为 null 的行各自成组。 */
  const dedupRows = (list) => {
    if (!backendDedup()) return list;
    const firstOfGroup = new Map();
    for (const r of [...list].sort((a, b) => a.id - b.id)) {
      const key = r.url ? `u:${r.url}` : `r:${r.id}`;
      if (!firstOfGroup.has(key)) firstOfGroup.set(key, r.id);
    }
    const keep = new Set(firstOfGroup.values());
    return list.filter((r) => keep.has(r.id));
  };
  const countsOf = (list) => ({
    total: list.length,
    unread: list.filter((r) => !r.is_read).length,
    starred: list.filter((r) => r.is_starred).length,
    today: list.filter((r) => localDayKey(Date.parse(r.published_at)) === localDayKey(NOW)).length,
  });
  const recomputeView = (layout) => countsOf(dedupRows(B.rows.filter((r) => feedLayout(r.feed_id) === layout)));
  const listRows = (a) => {
    let l = B.rows.filter((r) => inArgsScope(r, a));
    l = dedupRows(l);
    const dir = a.newest_first === false ? 1 : -1;
    l = [...l].sort((x, y) => {
      const bt = dir * (Date.parse(x.published_at) - Date.parse(y.published_at));
      return bt !== 0 ? bt : dir * (x.id - y.id);
    });
    if (a.last_published != null && a.last_id != null) {
      const ts = Date.parse(a.last_published);
      l = l.filter((r) => (dir < 0 ? Date.parse(r.published_at) < ts : Date.parse(r.published_at) > ts)
        || (Date.parse(r.published_at) === ts && (dir < 0 ? r.id < a.last_id : r.id > a.last_id)));
    }
    const off = a.offset ?? 0;
    const lim = a.limit == null ? l.length : a.limit;
    return l.slice(off, off + lim).map((r) => ({ ...r }));
  };
  const scopeDetailFor = (layout) => {
    const inLayout = B.rows.filter((r) => !layout || feedLayout(r.feed_id) === layout);
    const all = dedupRows(inLayout);
    return {
      all: countsOf(all),
      folders: FOLDERS.map((fo) => ({
        scope_id: fo.id,
        ...countsOf(dedupRows(all.filter((r) => (FEEDS.find((f) => f.id === r.feed_id) || {}).folder_id === fo.id))),
      })),
      feeds: FEEDS.map((f) => ({
        scope_id: f.id,
        ...countsOf(dedupRows(all.filter((r) => r.feed_id === f.id))),
      })),
    };
  };
  /* 读本地闭包（与 Rust set_read/mark_all_read 同构）：主行 + 同 url 未读副本。
     R2/N1：返回**真实翻转行回执** [{id, feed_id}]（与生产命令一致）。 */
  const markReadClosure = (seedIds) => {
    const receipts = [];
    const urls = new Set(seedIds.map((id) => (B.rows.find((r) => r.id === id) || {}).url).filter(Boolean));
    for (const r of B.rows) {
      const isSeed = seedIds.includes(r.id);
      const inGroup = r.url && urls.has(r.url);
      if ((isSeed || inGroup) && !r.is_read) {
        r.is_read = true;
        receipts.push({ id: r.id, feed_id: r.feed_id });
      }
    }
    return receipts;
  };
  const resetBackend = (rows, smartDedup) => {
    B.rows = rows;
    B.settingsRaw = JSON.stringify({ smartDedup });
    B.scopeCalls = [];
    B.deferCountsOnce = false;
    B.pendingCounts = [];
    B.deferReadId = null;
    B.pendingRead = [];
    B.deferStarId = null;
    B.pendingStar = [];
    B.deferGetSettingOnce = null;
    B.pendingGetSetting = [];
    B.rejectGetSettingOnce = false;
    B.rejectGetSettingCount = 0;
    B.getSettingCalls = 0;
    B.setSettingCalls = 0;
    B.setSettingPlan = null;
    B.setSettingPlans = [];
    B.pendingSetSetting = [];
    B.failNextCounts = false;
    B.listCalls = 0;
    B.getArticleCalls = 0;
  };

  const restoreInvoke = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
    S.invokeCalls.push({ cmd, args });
    switch (cmd) {
      case 'list_folders': return Promise.resolve(JSON.parse(JSON.stringify(FOLDERS)));
      case 'list_feeds': return Promise.resolve(JSON.parse(JSON.stringify(FEEDS)));
      case 'list_articles': { B.listCalls += 1; return Promise.resolve(listRows((args && args.args) || {})); }
      case 'article_index': {
        const a = (args && args.args) || {};
        const full = listRows({ ...a, limit: null, offset: 0, last_published: null, last_id: null });
        const pos = full.findIndex((r) => r.id === Number(args.articleId));
        return Promise.resolve(pos < 0 ? null : pos);
      }
      case 'get_article': {
        B.getArticleCalls += 1;
        const row = B.rows.find((r) => r.id === Number(args.id));
        return Promise.resolve(row ? { ...row, content_html: '<p>详情</p>' } : null);
      }
      /* R2/N2：计数组合快照是唯一计数 IPC（源键+聚合键同一时刻）。defer 计划
         用于受控屏障：载荷在**请求发起时**计算（模拟"旧读取"），手动放行。 */
      case 'counts_snapshot': {
        const layout = args && args.layout;
        B.scopeCalls.push(layout);
        const payload = {
          feeds: FEEDS.map((f) => ({ feed_id: f.id, ...countsOf(dedupRows(B.rows.filter((r) => r.feed_id === f.id))) })),
          scope: scopeDetailFor(layout),
        };
        if (B.failNextCounts) { B.failNextCounts = false; return Promise.reject({ message: '注入：counts_snapshot 失败' }); }
        if (B.deferCountsOnce) {
          B.deferCountsOnce = false;
          return new Promise((resolve) => { B.pendingCounts.push({ payload, release: () => resolve(payload) }); });
        }
        return Promise.resolve(payload);
      }
      case 'set_read': {
        const id = Number(args.id);
        if (args.read === true) {
          if (B.deferReadId === id) {
            B.deferReadId = null;
            return new Promise((resolve, reject) => {
              /* R4：提交/回应分离——commit() 只做后端提交（可提前），release()
                 结算回应；reject() 注入失败（回滚路径探针）。 */
              let receipt = null;
              const commit = () => { if (receipt === null) receipt = markReadClosure([id]); return receipt; };
              B.pendingRead.push({ commit, release: () => resolve(commit()), reject: () => reject({ message: '注入：标读失败' }) });
            });
          }
          return Promise.resolve(markReadClosure([id]));
        }
        const r = B.rows.find((x) => x.id === id);
        const changed = r && r.is_read ? [{ id, feed_id: r.feed_id }] : [];
        if (r) r.is_read = false;
        return Promise.resolve(changed);
      }
      case 'set_read_bulk': {
        if (args.read === true) {
          const ids = (args.ids ?? []).map(Number);
          return Promise.resolve(markReadClosure(ids));
        }
        const changed = [];
        for (const id of args.ids ?? []) {
          const r = B.rows.find((x) => x.id === Number(id));
          if (r && r.is_read) changed.push({ id: r.id, feed_id: r.feed_id });
          if (r) r.is_read = false;
        }
        return Promise.resolve(changed);
      }
      case 'set_starred': {
        const r = B.rows.find((x) => x.id === Number(args.id));
        if (r && args.starred === true && B.deferStarId === r.id) {
          B.deferStarId = null;
          return new Promise((resolve) => {
            B.pendingStar.push({ release: () => { r.is_starred = true; resolve(null); } });
          });
        }
        if (r) r.is_starred = args.starred === true;
        return Promise.resolve(null);
      }
      case 'mark_all_read': {
        const a = args || {};
        const scope = a.feedId != null ? String(a.feedId) : a.folderId != null ? `cat-${a.folderId}` : 'all';
        const eligible = B.rows.filter((r) => {
          if (a.layout && feedLayout(r.feed_id) !== a.layout) return false;
          if (scope.startsWith('cat-') && (FEEDS.find((f) => f.id === r.feed_id) || {}).folder_id !== Number(scope.slice(4))) return false;
          if (scope !== 'all' && !scope.startsWith('cat-') && String(r.feed_id) !== scope) return false;
          return true;
        });
        return Promise.resolve(markReadClosure(eligible.filter((r) => !r.is_read).map((r) => r.id)).length);
      }
      case 'get_setting': {
        B.getSettingCalls += 1;
        if (B.rejectGetSettingCount > 0) {
          B.rejectGetSettingCount -= 1;
          return Promise.reject({ message: '注入：读取设置失败' });
        }
        if (B.rejectGetSettingOnce) {
          B.rejectGetSettingOnce = false;
          return Promise.reject({ message: '注入：读取设置失败' });
        }
        /* R3：旧读探针——挂起一次并返回给定原文（读取时刻的载荷）。
           R4：挂起项可 release（结算）或 reject（注入读取失败——回读失败探针）。 */
        if (B.deferGetSettingOnce) {
          const raw = B.deferGetSettingOnce;
          B.deferGetSettingOnce = null;
          return new Promise((resolve, reject) => {
            B.pendingGetSetting.push({ release: () => resolve(raw), reject: () => reject({ message: '注入：读取设置失败' }) });
          });
        }
        return Promise.resolve(B.settingsRaw);
      }
      case 'set_setting': {
        B.setSettingCalls += 1;
        const plan = B.setSettingPlans.length ? B.setSettingPlans.shift() : B.setSettingPlan;
        B.setSettingPlan = null;
        if (plan && plan.mode === 'reject') {
          return Promise.reject({ message: '注入：设置保存失败' });
        }
        if (plan && plan.mode === 'manual') {
          /* R4：提交/回应分离——commit() 写持久态（可提前），release() 结算回应。 */
          return new Promise((resolve) => {
            let committed = false;
            const commit = () => { if (!committed) { committed = true; B.settingsRaw = args.value; } };
            B.pendingSetSetting.push({ args, commit, release: () => { commit(); resolve(null); } });
          });
        }
        if (plan && plan.mode === 'defer') {
          return new Promise((resolve) => {
            B.pendingSetSetting.push({ args, release: () => { B.settingsRaw = args.value; resolve(null); } });
          });
        }
        B.settingsRaw = args.value;
        return Promise.resolve(null);
      }
      case 'sync_status': return Promise.resolve({ connected: false });
      case 'sync_queue_stats': return Promise.resolve(null);
      default: return Promise.resolve(null);
    }
  });

  /** 捕获 toast 文案（与 ui-contracts 同型：临时替换 showToast）。 */
  const captureToasts = async (fn) => {
    const seen = [];
    const orig = store.getState().showToast;
    store.setState({ showToast: (t) => { seen.push(t); } });
    try { await fn(); } finally { store.setState({ showToast: orig }); }
    return seen;
  };

  try {
    /* 通用启动：写入持久态行 + 按开关置 store 设置 + bootstrap。 */
    const boot = async (rows, smartDedup) => {
      resetBackend(rows, smartDedup);
      await resetStore({ settings: { ...store.getState().settings, smartDedup } });
      await store.getState().bootstrapFromBackend();
    };

    /* ============================================================
       (A) markAll 闭包：同组副本全部读（store 侧数字真的清空，不依赖远端）
       ============================================================ */
    const shared = 'https://s.example/shared';
    const rowsA = [
      mkRow({ id: 1, feed_id: 10, url: shared, title: '副本1' }),
      mkRow({ id: 2, feed_id: 12, url: shared, title: '副本2' }),
      mkRow({ id: 3, feed_id: 20, url: shared, title: '副本3（social 布局，scope 外）' }),
      mkRow({ id: 4, feed_id: 10, url: 'https://s.example/other', title: '无关未读' }),
      mkRow({ id: 5, feed_id: 10, url: null, title: '无 URL 未读' }),
    ];
    await boot(rowsA, true);
    {
      const v = selectViewCounts(store.getState());
      checkNew('(dedup-a1) 开关开启：article 布局显示未读 = 折叠后 3（组 1 + 无关 1 + 无 URL 1）',
        v.unread === 3, JSON.stringify(v));
      store.setState({ activeViewFilter: 'all', timelineFilter: 'all', activeFeedFilter: 'all' });
      store.getState().markCurrentViewAllRead();
      await nTick(0); await nTick(0);
      checkNew('(dedup-a2) markAll 闭包：article scope 行 + 同组 social 副本全部读（不依赖远端收敛）',
        B.rows.every((r) => r.is_read), JSON.stringify(B.rows.map((r) => [r.id, r.is_read])));
      checkNew('(dedup-a3) markAll 后未读对账为 0（真实重算，非乐观推算）',
        selectViewCounts(store.getState()).unread === 0);
      checkNew('(dedup-a4) 列表（未读视图）同口径为空',
        listRows({ layout: 'article', only_unread: true }).length === 0);
    }

    /* ============================================================
       (B) 折叠组的非线性：单条 toggle 后数字 == 真实重算（无线性漂移）
       ============================================================ */
    const rowsB = [
      mkRow({ id: 11, feed_id: 10, url: 'https://s.example/g', title: '组A' }),
      mkRow({ id: 12, feed_id: 12, url: 'https://s.example/g', title: '组B' }),
    ];
    await boot(rowsB, true);
    {
      checkNew('(dedup-b1) 前置：组内两条未读折叠为 1', selectViewCounts(store.getState()).unread === 1);
      // 标读代表 → 后端闭包把两条都读；对账后必须为 0（线性推算只 -1）
      await store.getState().toggleEntryFlag('11', 'isRead');
      await nTick(0); await nTick(0);
      const v = selectViewCounts(store.getState());
      checkNew('(dedup-b2) 标读后数字 == 持久态重算（闭包使组内同读，非 -1）',
        B.rows.every((r) => r.is_read) && v.unread === recomputeView('article').unread && v.unread === 0);
      // 反向：拨回未读（只当前条目）→ 显示未读回 1
      await store.getState().toggleEntryFlag('11', 'isRead');
      await nTick(0); await nTick(0);
      const v2 = selectViewCounts(store.getState());
      checkNew('(dedup-b3) 取消标读只当前条目：数字回 1 == 重算（无累积漂移）',
        !B.rows[0].is_read && B.rows[1].is_read && v2.unread === recomputeView('article').unread && v2.unread === 1);
      await store.getState().toggleEntryFlag('11', 'isStarred');
      await nTick(0); await nTick(0);
      checkNew('(dedup-b4) 收藏数字 == 重算（同一对账源）',
        selectViewCounts(store.getState()).starred === recomputeView('article').starred);
    }

    /* ============================================================
       (C) 迟到计数快照：跨布局不得回填（主控探针场景，R2 单 IPC 版）
       ============================================================ */
    const rowsC = [
      mkRow({ id: 21, feed_id: 10, url: null, title: 'article 未读' }),
      mkRow({ id: 22, feed_id: 20, url: null, title: 'social 未读' }),
    ];
    await boot(rowsC, true);
    {
      B.deferCountsOnce = true;
      store.getState().markCurrentViewAllRead();
      await nTick(0); await nTick(0);
      checkNew('(dedup-c1) 前置：article 对账的 counts_snapshot 已被挂起（载荷=发起时刻）', B.pendingCounts.length === 1);
      store.getState().selectLayout('social');
      await nTick(0); await nTick(0); await nTick(0); await nTick(0);
      const before = selectViewCounts(store.getState()).all;
      B.pendingCounts.shift().release();
      await nTick(0); await nTick(0);
      const after = selectViewCounts(store.getState()).all;
      checkNew('(dedup-c2) 迟到计数快照被丢弃：social 数字不被 article 旧响应污染',
        before === after && store.getState().scopeCounts?.layout === 'social',
        JSON.stringify({ before, after, layout: store.getState().scopeCounts?.layout }));
    }

    /* ============================================================
       (D) 设置持久化顺序与失败回滚（P2-4）
       ============================================================ */
    await boot([mkRow({ id: 31, feed_id: 10, url: null, title: 'x' })], false);
    {
      B.setSettingPlan = { mode: 'defer' };
      B.listCalls = 0;
      store.getState().updateSettings({ smartDedup: true });
      await nTick(0); await nTick(0);
      checkNew('(dedup-d1) 落库未确认前不发起 reload（不按旧开关取数）',
        B.listCalls === 0 && B.pendingSetSetting.length === 1);
      B.pendingSetSetting.shift().release();
      await nTick(0); await nTick(0); await nTick(0);
      checkNew('(dedup-d2) 落库成功后按新口径 reload（list + scope_counts(layout)）',
        B.listCalls > 0 && B.scopeCalls.includes('article') && store.getState().scopeCounts?.layout === 'article');
      B.setSettingPlan = { mode: 'reject' };
      const toasts = await captureToasts(async () => {
        store.getState().updateSettings({ smartDedup: false });
        await nTick(0); await nTick(0);
      });
      checkNew('(dedup-d3) 写失败：设置回滚到已确认值（true）且失败可见',
        store.getState().settings.smartDedup === true && toasts.some((t) => String(t).includes('设置保存失败')),
        JSON.stringify({ smartDedup: store.getState().settings.smartDedup, toasts }));
    }
    /* R2/N3（主控探针等价）：接力场景——每笔成功推进确认基线（不把旧笔成功
       当过时）；跨笔保留 dedup 所需 reload（不只看最后 patch 含不含键）。 */
    {
      const { noteConfirmedAppSettings } = await import('../../dist-test/store/slices/settings.js');
      // 探针一：A(smartDedup=true) 成功 + B(fontSize) 失败 → 回滚目标 = A 的成功值
      await boot([mkRow({ id: 32, feed_id: 10, url: null })], false);
      noteConfirmedAppSettings(JSON.stringify(store.getState().settings));
      B.setSettingPlans = [{ mode: 'defer' }, { mode: 'reject' }];
      const toasts = await captureToasts(async () => {
        store.getState().updateSettings({ smartDedup: true });
        store.getState().updateSettings({ fontSize: 18 });
        await nTick(0);
        B.pendingSetSetting.shift().release(); // A 落库成功
        await nTick(0); await nTick(0); await nTick(0);
      });
      checkNew('(dedup-d4) 接力：A 成功 B 失败 → UI 回滚到 A 的已确认值（DB=true / UI=true，探针同型）',
        JSON.parse(B.settingsRaw).smartDedup === true && store.getState().settings.smartDedup === true,
        JSON.stringify({ persisted: JSON.parse(B.settingsRaw).smartDedup, ui: store.getState().settings.smartDedup, toasts }));
    }
    {
      // 探针二：A(dedup) + B(fontSize) 两笔全成功 → 至少发生一次 reload（不得被吞）
      await boot([mkRow({ id: 33, feed_id: 10, url: null })], false);
      let reloads = 0;
      const origReload = store.getState().reloadFromBackend;
      store.setState({ reloadFromBackend: async () => { reloads += 1; } });
      try {
        store.getState().updateSettings({ smartDedup: true });
        store.getState().updateSettings({ fontSize: 18 });
        await nTick(0); await nTick(0); await nTick(0);
      } finally {
        store.setState({ reloadFromBackend: origReload });
      }
      checkNew('(dedup-d5) 接力：A(dedup)+B(其他键) 全成功 → dedup 变更未被吞（reload ≥ 1）',
        JSON.parse(B.settingsRaw).smartDedup === true && reloads >= 1,
        JSON.stringify({ reloads, persisted: JSON.parse(B.settingsRaw).smartDedup }));
    }
    {
      const { noteConfirmedAppSettings } = await import('../../dist-test/store/slices/settings.js');
      // 探针三：前失败后成功；连续失败——确认基线不倒退、不串值
      await boot([mkRow({ id: 34, feed_id: 10, url: null })], false);
      /* 夹具确定性：前序探针改过 fontSize（18），这里显式钉住本探针的确认基线
         （16），否则断言 16 只是在依赖跨块残留值——基线必须来自本块声明。 */
      store.setState({ settings: { ...store.getState().settings, fontSize: 16 } });
      noteConfirmedAppSettings(JSON.stringify(store.getState().settings));
      B.setSettingPlans = [{ mode: 'reject' }];
      const t1 = await captureToasts(async () => {
        store.getState().updateSettings({ fontSize: 20 });
        await nTick(0); await nTick(0);
      });
      checkNew('(dedup-d6) 前失败：回滚到确认基线（fontSize=16）且失败可见',
        store.getState().settings.fontSize === 16 && t1.some((t) => String(t).includes('设置保存失败')));
      store.getState().updateSettings({ smartDedup: true }); // 后成功
      await nTick(0); await nTick(0); await nTick(0);
      checkNew('(dedup-d7) 前失败后成功：成功笔推进确认基线（UI/DB 一致 true）',
        store.getState().settings.smartDedup === true && JSON.parse(B.settingsRaw).smartDedup === true);
      B.setSettingPlans = [{ mode: 'reject' }, { mode: 'reject' }];
      const t2 = await captureToasts(async () => {
        store.getState().updateSettings({ fontSize: 22 });
        store.getState().updateSettings({ fontSize: 24 });
        await nTick(0); await nTick(0); await nTick(0);
      });
      checkNew('(dedup-d8) 连续失败：两笔失败只回滚到已确认值（dedup 仍 true、fontSize 回 16），提示 ≥2',
        store.getState().settings.smartDedup === true
        && store.getState().settings.fontSize === 16
        && t2.filter((t) => String(t).includes('设置保存失败')).length >= 2,
        JSON.stringify({ fontSize: store.getState().settings.fontSize, toasts: t2.length }));
      // 旧读回不得回退已确认基线：成功写入后，迟到的 bootstrap 原文被忽略
      const { noteConfirmedAppSettings: note2 } = await import('../../dist-test/store/slices/settings.js');
      note2(JSON.stringify({ ...store.getState().settings, smartDedup: false }));
      B.setSettingPlans = [{ mode: 'reject' }];
      await captureToasts(async () => {
        store.getState().updateSettings({ fontSize: 26 });
        await nTick(0); await nTick(0);
      });
      checkNew('(dedup-d9) 迟到的旧读回不得回退确认基线（失败后 UI/DB 仍 smartDedup=true）',
        store.getState().settings.smartDedup === true && JSON.parse(B.settingsRaw).smartDedup === true);
    }

    /* ============================================================
       (E) 搜索结果可真实打开：不在当前列表显示集合的行走按 id 打开回退
       ============================================================ */
    await boot([
      mkRow({ id: 41, feed_id: 10, url: null, title: 'article 行' }),
      mkRow({ id: 42, feed_id: 20, url: null, title: 'social 行（当前布局不可见）' }),
    ], false);
    {
      B.getArticleCalls = 0;
      await store.getState().anchorToArticle('42');
      await nTick(0);
      checkNew('(dedup-e1) 位置缺失时按 id 打开（get_article + activeArticleId 落地）',
        store.getState().activeArticleId === '42' && B.getArticleCalls >= 1,
        JSON.stringify({ active: store.getState().activeArticleId, calls: B.getArticleCalls }));
      checkNew('(dedup-e2) 视图行被注入（阅读器可渲染）',
        store.getState().entries.some((e) => e.id === '42'));
      await store.getState().anchorToArticle('41');
      await nTick(0);
      checkNew('(dedup-e3) 可见行照常锚定打开', store.getState().activeArticleId === '41');
    }

    /* ============================================================
       (F) 对账失败保守：数字保持（不推幻值、不抛未处理拒绝）
       ============================================================ */
    await boot([mkRow({ id: 51, feed_id: 10, url: null, title: 'y' })], true);
    {
      const before = selectViewCounts(store.getState()).unread;
      B.failNextCounts = true;
      await store.getState().toggleEntryFlag('51', 'isRead');
      await nTick(0); await nTick(0);
      const view = selectViewCounts(store.getState());
      checkNew('(dedup-f1) 对账读取失败：保留最后一次已知值（可解释的滞后，无崩溃）',
        B.rows[0].is_read === true && view.unread === before, JSON.stringify({ before, view }));
      store.setState({ timelineFilter: 'all' });
      const tree = selectTreeCounts(store.getState());
      checkNew('(dedup-f2) 树角标「全部」与视图角标同源（同值）',
        tree.get('all') === selectViewCounts(store.getState()).all);
    }

    /* ============================================================
       (G) N1：开关**关闭**（默认）时读闭包的本地收敛——
       回执行驱动 entries 兄弟 + 按源计数（不是只重取计数，也不靠 reload）
       ============================================================ */
    const gUrl = 'https://s.example/g1';
    const rowsG = [
      mkRow({ id: 61, feed_id: 10, url: gUrl, title: '组A' }),
      mkRow({ id: 62, feed_id: 12, url: gUrl, title: '组B（同窗口内，被闭包一并标读）' }),
      mkRow({ id: 63, feed_id: 20, url: gUrl, title: '组C（social 布局，窗口外同文副本）' }),
    ];
    await boot(rowsG, false);
    {
      const fcBefore = (id) => store.getState().feedCounts.get(String(id))?.unread;
      await store.getState().toggleEntryFlag('61', 'isRead');
      await nTick(0); await nTick(0);
      const st = store.getState();
      const bEntry = st.entries.find((e) => e.id === '62');
      checkNew('(dedup-g1) 开关关闭：单条标读后同窗口兄弟条目本地收敛为已读（回执行，非 reload）',
        B.rows.slice(0, 3).every((r) => r.is_read) && bEntry?.isRead === true,
        JSON.stringify({ rowReads: B.rows.map((r) => [r.id, r.is_read]), b: bEntry?.isRead }));
      checkNew('(dedup-g2) 源键按回执行逐行扣减（源12 未读 1→0；窗口外同文副本所在源20 同样收敛）',
        fcBefore(12) === 0 && fcBefore(20) === 0,
        JSON.stringify({ f10: fcBefore(10), f12: fcBefore(12), f20: fcBefore(20) }));
      // 批量路径同口径
      await boot([
        mkRow({ id: 64, feed_id: 10, url: 'https://s.example/g2' }),
        mkRow({ id: 65, feed_id: 12, url: 'https://s.example/g2' }),
      ], false);
      store.getState().markEntriesReadBulk(['64']);
      await nTick(0); await nTick(0);
      checkNew('(dedup-g3) 批量标读：回执使兄弟条目与双方源计数收敛',
        store.getState().entries.find((e) => e.id === '65')?.isRead === true
        && store.getState().feedCounts.get('10')?.unread === 0
        && store.getState().feedCounts.get('12')?.unread === 0);
    }

    /* ============================================================
       (H) N2：双写者共享提交协议——旧读取不得覆盖新对账 / 新读取
       （两方向都用受控屏障；旧载荷在发起时刻冻结）
       ============================================================ */
    {
      // 方向①：reload（旧）在途 → 对账（新）先落地 → 旧 reload 放行不得覆盖
      await boot([mkRow({ id: 71, feed_id: 10, url: null })], true);
      B.deferCountsOnce = true;
      const reloadP = store.getState().reloadFromBackend();
      await nTick(0); await nTick(0);
      checkNew('(dedup-h1) 前置：旧 reload 的计数读取已挂起（载荷=发起时刻）', B.pendingCounts.length === 1);
      await store.getState().toggleEntryFlag('71', 'isRead'); // 后端行翻转 + 其自身对账落地
      await nTick(0); await nTick(0);
      const fresh = selectViewCounts(store.getState()).unread;
      B.pendingCounts.shift().release();
      await reloadP;
      await nTick(0); await nTick(0);
      checkNew('(dedup-h2) 旧 reload 落地不得覆盖更新的对账（提交令牌）',
        selectViewCounts(store.getState()).unread === fresh && fresh === 0,
        JSON.stringify({ fresh, after: selectViewCounts(store.getState()).unread }));
    }
    {
      // 方向②：对账（旧）在途 → reload（新）落地 → 旧对账放行不得覆盖
      await boot([mkRow({ id: 72, feed_id: 10, url: null })], true);
      B.deferCountsOnce = true;
      await store.getState().toggleEntryFlag('72', 'isRead'); // 其自身对账被挂起（旧）
      await nTick(0); await nTick(0);
      checkNew('(dedup-h3) 前置：旧对账已挂起', B.pendingCounts.length === 1);
      await store.getState().reloadFromBackend(); // 新读取落地
      await nTick(0); await nTick(0);
      const fresh = selectViewCounts(store.getState()).unread;
      B.pendingCounts.shift().release();
      await nTick(0); await nTick(0);
      checkNew('(dedup-h4) 旧对账放行不得覆盖更新的 reload 快照',
        selectViewCounts(store.getState()).unread === fresh && fresh === 0,
        JSON.stringify({ fresh, after: selectViewCounts(store.getState()).unread }));
    }
    {
      // 方向③：未提交意图在途 → 期间任何计数快照不得落地覆盖（N2 明确要求）
      await boot([
        mkRow({ id: 73, feed_id: 10, url: null }),
        mkRow({ id: 74, feed_id: 12, url: null }),
      ], true);
      B.deferReadId = 73;
      store.getState().toggleEntryFlag('73', 'isRead'); // set_read 挂起（意图未提交）
      await nTick(0);
      checkNew('(dedup-h5) 前置：读写入在途（意图未提交）', B.pendingRead.length === 1);
      await store.getState().toggleEntryFlag('74', 'isRead'); // 其自身对账在途，须被 73 的未提交意图挡住
      await nTick(0); await nTick(0);
      const mid = selectViewCounts(store.getState()).unread;
      /* R1/P2-1 起聚合角标不做线性推算（折叠组非线性）——在途窗口保持**上一次
         权威值** 2；判别证据是 mid !== 1：若在途意图守卫缺失，74 的对账会以
         后端口径落地（73 尚未落库 ⇒ 快照值 1）而盖掉未提交意图。收敛由 h7
         的提交后对账完成（0）。 */
      checkNew('(dedup-h6) 在途意图期间：后到的计数快照被挡（保持上一次权威值 2，不落后端口径 1）',
        mid === 2, JSON.stringify({ mid }));
      B.pendingRead.shift().release();
      await nTick(0); await nTick(0);
      checkNew('(dedup-h7) 意图提交后：其自身对账落地为真实值（0）',
        selectViewCounts(store.getState()).unread === 0 && B.rows.every((r) => r.is_read));
    }
    {
      /* 方向④（R2 补测，主控要求「读/藏多请求在途」）：未提交意图的在途计数是
         **引用计数**而非布尔——两笔同时挂起时快照不得落地，只释放其中一笔仍
         不得落地（否则会以后端口径 1 覆盖另一笔未提交意图），全部落定后由最后
         一笔自身的对账收敛。若实现退化为布尔/少减，i3 会以 1 判红。 */
      await boot([
        mkRow({ id: 75, feed_id: 10, url: null }),
        mkRow({ id: 76, feed_id: 12, url: null }),
      ], true);
      const bootUnread = selectViewCounts(store.getState()).unread;
      checkNew('(dedup-i0) 前置：两行未读（权威快照 = 2）', bootUnread === 2, JSON.stringify({ bootUnread }));
      B.deferReadId = 75;  // 75 的 set_read 挂起（未提交）
      B.deferStarId = 76;  // 76 的 set_starred 挂起（未提交）
      store.getState().toggleEntryFlag('75', 'isRead');
      store.getState().toggleEntryFlag('76', 'isStarred');
      await nTick(0);
      checkNew('(dedup-i1) 前置：读/藏两笔写入同时在途（意图均未提交）',
        B.pendingRead.length === 1 && B.pendingStar.length === 1,
        JSON.stringify({ read: B.pendingRead.length, star: B.pendingStar.length }));
      // 第三笔写（76 标读）立即提交 → 其自身对账在途，必须被两笔未提交意图挡下
      store.getState().toggleEntryFlag('76', 'isRead');
      await nTick(0); await nTick(0);
      checkNew('(dedup-i2) 两笔未提交意图期间：第三笔的对账不得落地（保持权威值 2）',
        selectViewCounts(store.getState()).unread === 2,
        JSON.stringify({ unread: selectViewCounts(store.getState()).unread }));
      // 只释放读：仍有一笔（收藏）未提交 → 其触发的新对账同样不得落地
      B.pendingRead.shift().release();
      await nTick(0); await nTick(0);
      checkNew('(dedup-i3) 只释放一笔（收藏仍在途）：新对账仍被挡（引用计数未归零）',
        selectViewCounts(store.getState()).unread === 2,
        JSON.stringify({ unread: selectViewCounts(store.getState()).unread }));
      // 全部落定：最后一笔的对账以后端真实重算收敛（75 读、76 读+藏）
      B.pendingStar.shift().release();
      await nTick(0); await nTick(0);
      const finalCounts = selectViewCounts(store.getState());
      checkNew('(dedup-i4) 全部意图落定后：真实重算收敛（unread=0、starred=1）',
        finalCounts.unread === 0 && finalCounts.starred === 1 && B.rows.every((r) => r.is_read) && B.rows.find((r) => r.id === 76)?.is_starred === true,
        JSON.stringify({ finalCounts, rows: B.rows.map((r) => [r.id, r.is_read, r.is_starred]) }));
    }

    /* ============================================================
       (J) R3/P2-1：回执行逐行凭据——无关 URL 组的后续写不使整批回执失效；
           被较新 unread 接管的回执行不翻状态、计数不无条件扣。
       ============================================================ */
    {
      const U1 = 'https://r3.example/u1';
      await boot([
        mkRow({ id: 61, feed_id: 10, url: U1, title: '组U-主' }),
        mkRow({ id: 62, feed_id: 12, url: U1, title: '组U-兄弟' }),
        mkRow({ id: 63, feed_id: 10, url: 'https://r3.example/v', title: '组V-无关' }),
      ], false);
      B.deferReadId = 61;
      store.getState().toggleEntryFlag('61', 'isRead'); // 闭包请求（回执将含 62）挂起
      await nTick(0);
      checkNew('(dedup-j1) 前置：闭包读请求在途', B.pendingRead.length === 1);
      store.getState().toggleEntryFlag('63', 'isRead'); // 无关 URL 组：立即落定（全局写前沿推进）
      await nTick(0); await nTick(0);
      B.pendingRead.shift().release(); // 放行 61 的回执（后端闭包含 62）
      await nTick(0); await nTick(0);
      checkNew('(dedup-j2) 无关组的后续写不得使整批回执行失效：62 按回执翻已读（逐行凭据，非全局一刀切）',
        store.getState().entries.find((e) => e.id === '62')?.isRead === true);
      checkNew('(dedup-j3) 计数随实际应用的行扣减（源12 未读 1→0），数字与列表一致',
        store.getState().feedCounts.get('12')?.unread === 0
        && store.getState().entries.find((e) => e.id === '62')?.isRead === true);
    }
    {
      const U2 = 'https://r3.example/u2';
      await boot([
        mkRow({ id: 64, feed_id: 10, url: U2, title: '组W-主' }),
        mkRow({ id: 65, feed_id: 12, url: U2, title: '组W-兄弟' }),
      ], false);
      B.deferReadId = 64;
      store.getState().toggleEntryFlag('64', 'isRead'); // 挂起
      await nTick(0);
      /* 用户在窗口内把回执行本身改回未读（读→未读两次 toggle，请求均立即提交）。 */
      store.getState().toggleEntryFlag('65', 'isRead');
      await nTick(0);
      store.getState().toggleEntryFlag('65', 'isRead'); // 翻回未读
      await nTick(0); await nTick(0);
      const before = store.getState().feedCounts.get('12')?.unread;
      B.pendingRead.shift().release(); // 放行 64 的回执（请求时刻载荷含 65）
      await nTick(0); await nTick(0);
      checkNew('(dedup-j4) 已被较新 unread 接管的回执行：不翻状态、计数不无条件扣（欠计两侧都堵）',
        store.getState().entries.find((e) => e.id === '65')?.isRead === false
        && store.getState().feedCounts.get('12')?.unread === before,
        JSON.stringify({ read65: store.getState().entries.find((e) => e.id === '65')?.isRead, before, after: store.getState().feedCounts.get('12')?.unread }));
    }

    /* ============================================================
       (K) R3/P2-2：旧 reload 落地不得覆**已提交**的读/藏（计数与行级字段）。
           旧响应读取先于写、落地后于写；在途计数已归零，只有「读取时刻写
           前沿」判据能挡。
       ============================================================ */
    {
      await boot([mkRow({ id: 71, feed_id: 10, url: null, title: 'reload 竞态' })], false);
      B.deferCountsOnce = true;
      const reloadP = store.getState().reloadFromBackend(); // entries 快照先于写；counts 挂起
      await nTick(0); await nTick(0);
      checkNew('(dedup-k1) 前置：旧 reload 的计数读取已挂起', B.pendingCounts.length === 1);
      store.getState().toggleEntryFlag('71', 'isStarred'); // 提交成功（dedup 关：无对账兜底）
      await nTick(0); await nTick(0);
      checkNew('(dedup-k2) 前置：收藏已提交且乐观计数 +1',
        store.getState().feedCounts.get('10')?.starred === 1
        && store.getState().entries.find((e) => e.id === '71')?.isStarred === true);
      B.pendingCounts.shift().release(); // 放行旧计数（starred=0）
      await reloadP;
      await nTick(0); await nTick(0);
      checkNew('(dedup-k3) 旧 reload 的计数不得覆盖已提交写（starred 保持 1）',
        store.getState().feedCounts.get('10')?.starred === 1,
        JSON.stringify({ starred: store.getState().feedCounts.get('10')?.starred }));
      checkNew('(dedup-k4) 旧 reload 的行级快照不得把已提交收藏翻回（字段保留）',
        store.getState().entries.find((e) => e.id === '71')?.isStarred === true);
    }
    {
      /* 顺序另一向：写先提交 → 之后**新发起**的读取照常接受外部状态
         （新鲜读取不被本地值永久阻挡——不信任本地到底）。 */
      await boot([mkRow({ id: 72, feed_id: 10, url: null, title: '新鲜读取' })], false);
      store.getState().toggleEntryFlag('72', 'isStarred');
      await nTick(0); await nTick(0);
      B.rows.push(mkRow({ id: 73, feed_id: 10, url: null, title: '外部新增收藏', is_starred: true }));
      const fresh = store.getState().reloadFromBackend();
      await fresh;
      await nTick(0); await nTick(0);
      checkNew('(dedup-l1) 写之后发起的新读取接受外部状态（73 收藏入库、计数 = 2）',
        store.getState().entries.find((e) => e.id === '73')?.isStarred === true
        && store.getState().feedCounts.get('10')?.starred === 2,
        JSON.stringify({ starred: store.getState().feedCounts.get('10')?.starred }));
    }
    {
      /* 多笔在途（读+藏同时挂起）→ 旧 reload 落地：两字段各自保留、计数不落。 */
      await boot([
        mkRow({ id: 74, feed_id: 10, url: null, title: '多笔读' }),
        mkRow({ id: 75, feed_id: 12, url: null, title: '多笔藏' }),
      ], false);
      B.deferCountsOnce = true;
      const reloadP = store.getState().reloadFromBackend();
      await nTick(0); await nTick(0);
      B.deferReadId = 74;
      B.deferStarId = 75;
      store.getState().toggleEntryFlag('74', 'isRead'); // 读在途（未提交）
      store.getState().toggleEntryFlag('75', 'isStarred'); // 藏在途（未提交）
      await nTick(0);
      const midUnread = store.getState().feedCounts.get('10')?.unread;
      const midStarred = store.getState().feedCounts.get('12')?.starred;
      B.pendingCounts.shift().release();
      await reloadP;
      await nTick(0); await nTick(0);
      checkNew('(dedup-l2) 多笔未提交意图期间旧 reload 计数不落地（乐观值保持）',
        store.getState().feedCounts.get('10')?.unread === midUnread
        && store.getState().feedCounts.get('12')?.starred === midStarred);
      checkNew('(dedup-l3) 多笔在途的行级字段保留（读/藏乐观值各归各行，不被旧快照翻回）',
        store.getState().entries.find((e) => e.id === '74')?.isRead === true
        && store.getState().entries.find((e) => e.id === '75')?.isStarred === true);
      // 收尾：释放两笔在途写，不留悬挂
      B.pendingRead.shift().release();
      B.pendingStar.shift().release();
      await nTick(0); await nTick(0);
    }

    /* ============================================================
       (M) R3/P2-N3：主控探针 A 等价（controller-settings-config-r2.mjs）——
           成功写之后的**新鲜读**刷新确认基线；下一笔失败回滚到读回值。
       ============================================================ */
    {
      await boot([mkRow({ id: 81, feed_id: 10, url: null })], false);
      await store.getState().bootstrapSettings(); // 初始读取（基线 = 原文）
      await nTick(0); await nTick(0);
      store.getState().updateSettings({ fontSize: 18 });
      await nTick(0); await nTick(0);
      /* 外部配置：持久态 smartDedup=true（模拟 configSyncApply / 手改 DB）。 */
      B.settingsRaw = JSON.stringify({ ...JSON.parse(B.settingsRaw), smartDedup: true });
      await store.getState().bootstrapSettings(); // 新鲜读：无写交错
      await nTick(0); await nTick(0);
      const afterApply = store.getState().settings.smartDedup;
      B.setSettingPlan = { mode: 'reject' };
      const toasts = await captureToasts(async () => {
        store.getState().updateSettings({ fontSize: 19 });
        await nTick(0); await nTick(0);
      });
      checkNew('(dedup-m1) 成功写之后的新鲜读刷新确认基线：下一笔失败回滚到导入值（UI/DB 一致 true）',
        afterApply === true && store.getState().settings.smartDedup === true
        && JSON.parse(B.settingsRaw).smartDedup === true
        && toasts.some((t) => String(t).includes('设置保存失败')),
        JSON.stringify({ afterApply, ui: store.getState().settings.smartDedup, persisted: JSON.parse(B.settingsRaw).smartDedup }));
      checkNew('(dedup-m2) 回滚目标 = 读回后的基线（fontSize 回 18，而非导入前值）',
        store.getState().settings.fontSize === 18,
        JSON.stringify({ fontSize: store.getState().settings.fontSize }));
    }

    /* ============================================================
       (N) R3/P2-N3：主控探针 B 等价（controller-settings-stale-r2.mjs）——
           旧读取挂起期间的成功写，被放行后的旧读不得倒灌。
       ============================================================ */
    {
      await boot([mkRow({ id: 82, feed_id: 10, url: null })], false);
      const oldRaw = B.settingsRaw; // 读取时刻的原文（smartDedup=false）
      B.deferGetSettingOnce = oldRaw;
      const reading = store.getState().bootstrapSettings();
      await nTick(0);
      checkNew('(dedup-n1) 前置：旧 bootstrap 读取已挂起', B.pendingGetSetting.length === 1);
      store.getState().updateSettings({ smartDedup: true }); // 成功写先落
      await nTick(0); await nTick(0);
      B.pendingGetSetting.shift().release();
      await reading;
      await nTick(0); await nTick(0);
      checkNew('(dedup-n2) 放行旧读不得倒灌已成功写：UI 保持 true 与 DB 一致',
        store.getState().settings.smartDedup === true
        && JSON.parse(B.settingsRaw).smartDedup === true,
        JSON.stringify({ ui: store.getState().settings.smartDedup, persisted: JSON.parse(B.settingsRaw).smartDedup }));
    }

    /* ============================================================
       (O) R3：配置应用边界（store 协调面）——窗口内新点击排在应用+回读后；
           回读保留点击键、其余取远端；载荷发送时重建；确认基线以后端为准。
       ============================================================ */
    {
      const { runAppSettingsApplyBoundary, readBackAppliedAppSettings } = await import('../../dist-test/store/slices/settings.js');
      await boot([mkRow({ id: 83, feed_id: 10, url: null })], false);
      B.setSettingPlan = { mode: 'defer' };
      /* 应用回读用受控挂起：门在回读在途期间保持——保证「窗口内点击」确实
         发生在应用+回读完成之前（否则读回先落地，点击不再是窗口内语义）。 */
      const remoteRaw = JSON.stringify({ ...JSON.parse(B.settingsRaw), smartDedup: true, fontSize: 22 });
      B.deferGetSettingOnce = remoteRaw;
      const applyDone = runAppSettingsApplyBoundary(async () => {
        await readBackAppliedAppSettings();
      });
      await nTick(0); await nTick(0);
      checkNew('(dedup-o0) 前置：应用回读已挂起（门未释放）', B.pendingGetSetting.length === 1);
      store.getState().updateSettings({ fontSize: 18 }); // 窗口内新点击
      await nTick(0); await nTick(0);
      checkNew('(dedup-o1) 应用期间新点击乐观可见且请求未发出（排门后）',
        store.getState().settings.fontSize === 18 && B.pendingSetSetting.length === 0,
        JSON.stringify({ fontSize: store.getState().settings.fontSize, sent: B.pendingSetSetting.length }));
      B.pendingGetSetting.shift().release(); // 放行回读（远端：dedup true / fontSize 22）
      await applyDone;
      await nTick(0); await nTick(0);
      checkNew('(dedup-o2) 回读保留点击键、其余取远端（点击不丢、远端覆盖生效）',
        store.getState().settings.smartDedup === true && store.getState().settings.fontSize === 18,
        JSON.stringify({ dedup: store.getState().settings.smartDedup, fontSize: store.getState().settings.fontSize }));
      checkNew('(dedup-o3) 门释放后点击请求才发出，载荷含回读基线 + 点击（发送时重建）',
        B.pendingSetSetting.length === 1
        && JSON.parse(B.pendingSetSetting[0].args.value).fontSize === 18
        && JSON.parse(B.pendingSetSetting[0].args.value).smartDedup === true,
        JSON.stringify({ sent: B.pendingSetSetting.length }));
      B.pendingSetSetting.shift().release();
      await nTick(0); await nTick(0);
      checkNew('(dedup-o4) 点击落库成功：UI/DB 一致（fontSize 18 / dedup true）',
        JSON.parse(B.settingsRaw).fontSize === 18 && JSON.parse(B.settingsRaw).smartDedup === true
        && store.getState().settings.fontSize === 18);
    }

    /* ============================================================
       (P) R3/P3-6：回执→视图缓存——切走再切回，兄弟条目保持已读
           （缓存内容 + 真实恢复双取证）。
       ============================================================ */
    {
      const U = 'https://r3.example/cache-u';
      await boot([
        mkRow({ id: 91, feed_id: 10, url: U, title: '缓存-主' }),
        mkRow({ id: 92, feed_id: 12, url: U, title: '缓存-兄弟' }),
      ], false);
      store.getState().toggleEntryFlag('91', 'isRead');
      await nTick(0); await nTick(0);
      checkNew('(dedup-p1) 前置：回执已把兄弟 92 本地翻读', store.getState().entries.find((e) => e.id === '92')?.isRead === true);
      const { viewEntriesCache } = await import('../../dist-test/store/internals.js');
      const cached = viewEntriesCache.get('article|all|all');
      checkNew('(dedup-p2) 回执同步当前视图缓存（92 在缓存快照中也是已读）',
        cached?.entries.find((e) => e.id === '92')?.isRead === true);
      store.getState().selectView('starred');
      await nTick(0); await nTick(0);
      store.getState().selectView('all');
      await nTick(0); await nTick(0); await nTick(0);
      checkNew('(dedup-p3) 切走再切回：兄弟条目保持已读（缓存恢复 + 后台刷新同口径）',
        store.getState().entries.find((e) => e.id === '92')?.isRead === true,
        JSON.stringify({ read92: store.getState().entries.find((e) => e.id === '92')?.isRead }));
    }

    /* ============================================================
       (Q) R4/P2-1：配置应用使**更早的启动读取**过期（读取凭据——readSerial
           后发起者优先）。主控探针 controller-apply-old-bootstrap-r3 等价：
           旧 false 读取挂起 → 应用 DB=true 并读回 UI=true → 放行旧读不得倒灌。
       ============================================================ */
    {
      const settingsMod = await import('../../dist-test/store/slices/settings.js');
      await boot([mkRow({ id: 84, feed_id: 10, url: null })], false);
      B.settingsRaw = JSON.stringify({ smartDedup: false });
      B.deferGetSettingOnce = JSON.stringify({ smartDedup: false }); // 旧读：读取时刻原文
      const oldRead = store.getState().bootstrapSettings();
      await nTick(0);
      checkNew('(dedup-q1) 前置：旧 bootstrap 读取挂起', B.pendingGetSetting.length === 1);
      await settingsMod.runAppSettingsApplyBoundary(async () => {
        B.settingsRaw = JSON.stringify({ smartDedup: true }); // 配置应用提交后的持久态
        await settingsMod.readBackAppliedAppSettings();
      });
      const afterApply = store.getState().settings.smartDedup;
      checkNew('(dedup-q2) 应用读回落地：UI=true', afterApply === true);
      B.pendingGetSetting.shift().release();
      await oldRead;
      await nTick(0); await nTick(0);
      checkNew('(dedup-q3) 放行更早的启动读取不得倒灌：UI=true 与 DB 一致（旧读被读取凭据失效）',
        store.getState().settings.smartDedup === true && JSON.parse(B.settingsRaw).smartDedup === true,
        JSON.stringify({ ui: store.getState().settings.smartDedup, persisted: JSON.parse(B.settingsRaw).smartDedup }));
    }

    /* ============================================================
       (R) R4/P2-2（设置面）：bootstrap 等待**捕获时**写前沿，读取不抢跑在途写
           （不依赖 IPC FIFO）；提交/回应分离三时序（已提交回应延迟 / 未提交
           成功收敛 / 未提交失败回滚）。
       ============================================================ */
    {
      await boot([mkRow({ id: 85, feed_id: 10, url: null })], false);
      B.settingsRaw = JSON.stringify({ fontSize: 16 });
      B.setSettingPlan = { mode: 'manual' };
      store.getState().updateSettings({ fontSize: 18 });
      await nTick(0);
      checkNew('(dedup-r1a) 前置：写已发出（manual 挂起）', B.pendingSetSetting.length === 1);
      B.pendingSetSetting[0].commit(); // 后端已提交（持久态=18），回应延迟
      const readsBefore = B.getSettingCalls;
      const bootP = store.getState().bootstrapSettings();
      await nTick(0); await nTick(0);
      checkNew('(dedup-r1b) 已提交回应延迟：bootstrap 等捕获时前沿，回应未结算前不发读取请求',
        B.getSettingCalls === readsBefore,
        JSON.stringify({ reads: B.getSettingCalls, before: readsBefore }));
      B.pendingSetSetting.shift().release();
      await bootP; await nTick(0); await nTick(0);
      checkNew('(dedup-r1c) 回应结算后读取新值并接受（UI/DB 一致 18）',
        B.getSettingCalls > readsBefore && store.getState().settings.fontSize === 18
        && JSON.parse(B.settingsRaw).fontSize === 18);
    }
    {
      await boot([mkRow({ id: 86, feed_id: 10, url: null })], false);
      B.settingsRaw = JSON.stringify({ fontSize: 16 });
      B.setSettingPlan = { mode: 'manual' };
      store.getState().updateSettings({ fontSize: 18 });
      await nTick(0);
      const readsAfterWrite = B.getSettingCalls;
      const bootP = store.getState().bootstrapSettings();
      await nTick(0); await nTick(0);
      checkNew('(dedup-r2a) 写未提交：bootstrap 挂起等待（未加读取）、点击意图保持 18',
        B.getSettingCalls === readsAfterWrite && store.getState().settings.fontSize === 18,
        JSON.stringify({ reads: B.getSettingCalls, afterWrite: readsAfterWrite, fontSize: store.getState().settings.fontSize }));
      B.pendingSetSetting.shift().release(); // 提交 + 回应
      await bootP; await nTick(0); await nTick(0);
      checkNew('(dedup-r2b) 写成功后读取新值收敛：UI=18 与 DB 一致',
        store.getState().settings.fontSize === 18 && JSON.parse(B.settingsRaw).fontSize === 18);
    }
    {
      await boot([mkRow({ id: 87, feed_id: 10, url: null })], false);
      B.settingsRaw = JSON.stringify({ fontSize: 16 });
      await store.getState().bootstrapSettings(); // 建立确认基线 16
      await nTick(0); await nTick(0);
      B.setSettingPlan = { mode: 'reject' };
      store.getState().updateSettings({ fontSize: 18 }); // 提交失败
      const bootP = store.getState().bootstrapSettings();
      await bootP; await nTick(0); await nTick(0);
      checkNew('(dedup-r3) 写失败回滚后读取（旧值=DB）：不残留未提交意图、UI/DB 一致 16',
        store.getState().settings.fontSize === 16 && JSON.parse(B.settingsRaw).fontSize === 16);
    }

    /* ============================================================
       (S) R4/P2-2（articles 面）：读取发起时已在途的行×字段保护——reload 快照
           不得把未落定乐观值翻回（不依赖 IPC FIFO）；成功收敛、失败回滚。
       ============================================================ */
    {
      await boot([mkRow({ id: 93, feed_id: 10, url: null, title: '在途读' })], false);
      B.deferReadId = 93;
      store.getState().toggleEntryFlag('93', 'isRead'); // 乐观读，请求在途
      await nTick(0);
      checkNew('(dedup-s1a) 前置：读请求在途（乐观已读）',
        B.pendingRead.length === 1 && store.getState().entries.find((e) => e.id === '93')?.isRead === true);
      B.deferCountsOnce = true;
      const reloadP = store.getState().reloadFromBackend(); // 快照读取发起（后端仍未读）
      await nTick(0); await nTick(0);
      B.pendingCounts.shift().release();
      await reloadP; await nTick(0); await nTick(0);
      checkNew('(dedup-s1b) 读前快照不得翻回在途写：条目保持乐观已读、计数不落地',
        store.getState().entries.find((e) => e.id === '93')?.isRead === true
        && store.getState().feedCounts.get('10')?.unread === 0,
        JSON.stringify({ read: store.getState().entries.find((e) => e.id === '93')?.isRead, unread: store.getState().feedCounts.get('10')?.unread }));
      B.pendingRead[0].commit(); // 后端提交（已读）
      B.pendingRead.shift().release(); // 回应结算
      await nTick(0); await nTick(0);
      checkNew('(dedup-s1c) 写成功：UI 与 DB 一致（已读、未读 0）',
        store.getState().entries.find((e) => e.id === '93')?.isRead === true
        && store.getState().feedCounts.get('10')?.unread === 0
        && B.rows.find((r) => r.id === 93)?.is_read === true);
      await store.getState().reloadFromBackend();
      await nTick(0); await nTick(0);
      checkNew('(dedup-s1d) 写落定后的新读取接受后端真值（仍已读）',
        store.getState().entries.find((e) => e.id === '93')?.isRead === true);
    }
    {
      await boot([mkRow({ id: 94, feed_id: 10, url: null, title: '在途读失败' })], false);
      B.deferReadId = 94;
      store.getState().toggleEntryFlag('94', 'isRead');
      await nTick(0);
      B.deferCountsOnce = true;
      const reloadP = store.getState().reloadFromBackend();
      await nTick(0); await nTick(0);
      B.pendingCounts.shift().release();
      await reloadP; await nTick(0); await nTick(0);
      checkNew('(dedup-s2a) 前置：快照未翻回（乐观已读、计数 0）',
        store.getState().entries.find((e) => e.id === '94')?.isRead === true
        && store.getState().feedCounts.get('10')?.unread === 0);
      B.pendingRead.shift().reject(); // 写失败
      await nTick(0); await nTick(0);
      checkNew('(dedup-s2b) 失败回滚：读态恢复未读、未读计数回 1（快照未 bump 保护字段版本，回滚有效）',
        store.getState().entries.find((e) => e.id === '94')?.isRead === false
        && store.getState().feedCounts.get('10')?.unread === 1,
        JSON.stringify({ read: store.getState().entries.find((e) => e.id === '94')?.isRead, unread: store.getState().feedCounts.get('10')?.unread }));
    }

    /* ============================================================
       (T) R4/P2-3：门后多点击——发送载荷剔除**更晚且未落定**的意图：
          旧请求不得偷提交后续失败意图（跨字段 A成功B失败；同字段相反另块）。
       ============================================================ */
    {
      const { runAppSettingsApplyBoundary, readBackAppliedAppSettings } = await import('../../dist-test/store/slices/settings.js');
      await boot([mkRow({ id: 88, feed_id: 10, url: null })], false);
      /* 夹具确定性：boot 只覆盖 smartDedup——显式钉住本块的三键基线（主题/
         字号会被跨域残留带入，t3/t4 的"排除更晚意图"依赖 B.prev 的准确值）。 */
      store.setState({ settings: { ...store.getState().settings, smartDedup: false, fontSize: 16, themeMode: 'dark' } });
      B.settingsRaw = JSON.stringify({ ...store.getState().settings, smartDedup: false, fontSize: 16, themeMode: 'dark' });
      B.setSettingPlans = [{ mode: 'manual' }, { mode: 'reject' }];
      B.deferGetSettingOnce = JSON.stringify({ ...store.getState().settings, smartDedup: true, fontSize: 22 });
      const applyP = runAppSettingsApplyBoundary(async () => { await readBackAppliedAppSettings(); });
      await nTick(0); await nTick(0);
      store.getState().updateSettings({ fontSize: 18 });        // A（窗口内）
      store.getState().updateSettings({ themeMode: 'light' });  // B（窗口内，更晚）
      await nTick(0);
      checkNew('(dedup-t1) 前置：A/B 乐观可见、请求均未发出（门后）',
        store.getState().settings.fontSize === 18 && store.getState().settings.themeMode === 'light'
        && B.pendingSetSetting.length === 0);
      B.pendingGetSetting.shift().release(); // 回读：远端 dedup=true（点击键保留）
      await applyP; await nTick(0); await nTick(0);
      checkNew('(dedup-t2) 回读保留点击键：fontSize=18/themeMode=light、dedup=true',
        store.getState().settings.fontSize === 18 && store.getState().settings.themeMode === 'light'
        && store.getState().settings.smartDedup === true);
      checkNew('(dedup-t3) A 载荷剔除更晚未落定的 B 意图：themeMode 仍为 dark、含远端 dedup=true',
        B.pendingSetSetting.length === 1
        && JSON.parse(B.pendingSetSetting[0].args.value).fontSize === 18
        && JSON.parse(B.pendingSetSetting[0].args.value).themeMode === 'dark'
        && JSON.parse(B.pendingSetSetting[0].args.value).smartDedup === true,
        JSON.stringify({ sent: B.pendingSetSetting.length }));
      B.pendingSetSetting[0].commit();
      B.pendingSetSetting.shift().release(); // A 成功（DB=A 载荷）
      const tToasts = await captureToasts(async () => {
        await nTick(0); await nTick(0); await nTick(0); // B 发出 → reject
      });
      checkNew('(dedup-t4) B 失败不得污染 DB/UI：回滚到确认基线（A 载荷）——UI/DB 一致',
        store.getState().settings.themeMode === 'dark' && JSON.parse(B.settingsRaw).themeMode === 'dark'
        && store.getState().settings.fontSize === 18 && JSON.parse(B.settingsRaw).fontSize === 18
        && store.getState().settings.smartDedup === true
        && tToasts.some((t) => String(t).includes('设置保存失败')),
        JSON.stringify({ uiTheme: store.getState().settings.themeMode, dbTheme: JSON.parse(B.settingsRaw).themeMode }));
    }
    {
      // 同字段相反：A fontSize 18（manual 成功）→ B fontSize 16（reject 失败）
      const { runAppSettingsApplyBoundary, readBackAppliedAppSettings } = await import('../../dist-test/store/slices/settings.js');
      await boot([mkRow({ id: 95, feed_id: 10, url: null })], false);
      store.setState({ settings: { ...store.getState().settings, smartDedup: false, fontSize: 16 } });
      B.settingsRaw = JSON.stringify({ ...store.getState().settings, smartDedup: false, fontSize: 16 });
      B.setSettingPlans = [{ mode: 'manual' }, { mode: 'reject' }];
      B.deferGetSettingOnce = JSON.stringify({ ...store.getState().settings, smartDedup: false, fontSize: 22 });
      const applyP = runAppSettingsApplyBoundary(async () => { await readBackAppliedAppSettings(); });
      await nTick(0); await nTick(0);
      store.getState().updateSettings({ fontSize: 18 }); // A
      store.getState().updateSettings({ fontSize: 16 }); // B（同字段相反）
      await nTick(0);
      B.pendingGetSetting.shift().release();
      await applyP; await nTick(0); await nTick(0);
      checkNew('(dedup-t5) 同字段相反：A 载荷取 A 的 18（剔除 B 的 16）',
        B.pendingSetSetting.length === 1 && JSON.parse(B.pendingSetSetting[0].args.value).fontSize === 18,
        JSON.stringify({ sent: B.pendingSetSetting.length }));
      B.pendingSetSetting[0].commit();
      B.pendingSetSetting.shift().release(); // A 成功
      await captureToasts(async () => { await nTick(0); await nTick(0); await nTick(0); }); // B reject
      checkNew('(dedup-t6) B 失败回滚到 A：UI/DB 同值 18（B 的 16 未被偷提交）',
        store.getState().settings.fontSize === 18 && JSON.parse(B.settingsRaw).fontSize === 18);
    }

    /* ============================================================
       (U) R4/P2-4：应用回读失败后，门后写不得用旧全量快照覆盖未读回的远端键——
           发送前先校准（读当前后端原文再发）；校准失败则不发送、可见失败、
           不改库、点击键回滚。
       ============================================================ */
    {
      const settingsMod = await import('../../dist-test/store/slices/settings.js');
      await boot([mkRow({ id: 89, feed_id: 10, url: null })], false);
      /* 夹具确定性：跨块残留的 settings 会带进本块（boot 只覆盖 smartDedup）——
         显式钉住本块的可视/持久基线。 */
      store.setState({ settings: { ...store.getState().settings, smartDedup: false, fontSize: 16 } });
      B.settingsRaw = JSON.stringify({ smartDedup: false, fontSize: 16 });
      B.setSettingPlan = { mode: 'manual' }; // 点击写得受控挂起（校验载荷）
      B.deferGetSettingOnce = JSON.stringify({ smartDedup: false, fontSize: 16 }); // 回读（将失败）
      const applyP = settingsMod.runAppSettingsApplyBoundary(async () => {
        try { await settingsMod.readBackAppliedAppSettings(); } catch { settingsMod.markAppSettingsUnconfirmed(); }
      });
      await nTick(0); await nTick(0);
      store.getState().updateSettings({ fontSize: 18 }); // 窗口内只改 fontSize
      await nTick(0);
      checkNew('(dedup-u0) 前置：回读挂起、点击在窗口内且未发出',
        B.pendingGetSetting.length === 1 && B.pendingSetSetting.length === 0);
      /* 远端应用后的持久态（本地未读回）先就位——校准读将在回读失败后读它。 */
      B.settingsRaw = JSON.stringify({ smartDedup: true, fontSize: 22 });
      B.pendingGetSetting.shift().reject(); // 回读失败 → 基线未确认
      await applyP; await nTick(0); await nTick(0); await nTick(0);
      checkNew('(dedup-u1) 回读失败后发送前校准：载荷含远端 dedup=true（未读回的远端键不被旧快照覆写）',
        B.pendingSetSetting.length === 1
        && JSON.parse(B.pendingSetSetting[0].args.value).smartDedup === true
        && JSON.parse(B.pendingSetSetting[0].args.value).fontSize === 18
        && B.getSettingCalls >= 2,
        JSON.stringify({ sent: B.pendingSetSetting.length, reads: B.getSettingCalls }));
      B.pendingSetSetting.shift().release();
      await nTick(0); await nTick(0);
      checkNew('(dedup-u2) 校准后发送成功：UI/DB 一致（dedup=true / fontSize=18）',
        store.getState().settings.smartDedup === true && store.getState().settings.fontSize === 18
        && JSON.parse(B.settingsRaw).smartDedup === true && JSON.parse(B.settingsRaw).fontSize === 18);
    }
    {
      const settingsMod = await import('../../dist-test/store/slices/settings.js');
      await boot([mkRow({ id: 90, feed_id: 10, url: null })], false);
      /* 夹具确定性：显式钉住本块基线与点击前值（跨块残留不清空 settings）。 */
      store.setState({ settings: { ...store.getState().settings, smartDedup: false, fontSize: 16 } });
      B.settingsRaw = JSON.stringify({ smartDedup: false, fontSize: 16 });
      B.deferGetSettingOnce = JSON.stringify({ smartDedup: false, fontSize: 16 }); // 回读（将失败）
      const applyP = settingsMod.runAppSettingsApplyBoundary(async () => {
        try { await settingsMod.readBackAppliedAppSettings(); } catch { settingsMod.markAppSettingsUnconfirmed(); }
      });
      await nTick(0); await nTick(0);
      store.getState().updateSettings({ fontSize: 18 });
      await nTick(0);
      B.rejectGetSettingOnce = true; // 校准（门释放后的下一次 get_setting）也失败
      B.pendingGetSetting.shift().reject(); // 回读失败
      const uToasts = await captureToasts(async () => { await applyP; await nTick(0); await nTick(0); await nTick(0); });
      checkNew('(dedup-u3) 校准失败：不发送（无 set_setting）、不改库、点击键回滚、失败可见',
        B.setSettingCalls === 0 && JSON.parse(B.settingsRaw).fontSize === 16
        && store.getState().settings.fontSize === 16
        && uToasts.some((t) => String(t).includes('配置基线未确认')),
        JSON.stringify({ calls: B.setSettingCalls, fontSize: store.getState().settings.fontSize, toasts: uToasts }));
    }

    /* ============================================================
       (V) R5/反例一（controller-remote-base-r4 等价）：远端新值 ≠ 旧本地值——
           载荷=「基线+本笔 patch」，被更晚失败意图触及的键取**远端基线**而非
           乐观前旧本值。附 A 失败 B 成功（失败意图不被后续笔偷提交）。
       ============================================================ */
    {
      const settingsMod = await import('../../dist-test/store/slices/settings.js');
      await boot([mkRow({ id: 96, feed_id: 10, url: null })], false);
      store.setState({ settings: { ...store.getState().settings, fontSize: 16, lineHeight: 180, smartDedup: false } });
      settingsMod.noteConfirmedAppSettings(JSON.stringify(store.getState().settings)); // 旧基线（lineHeight 180）
      B.setSettingPlans = [{ mode: 'manual' }, { mode: 'reject' }];
      B.deferGetSettingOnce = JSON.stringify({ ...store.getState().settings, lineHeight: 200, smartDedup: true }); // 远端新值
      const gate = mkDeferred();
      let enter; const entered = new Promise((r) => { enter = r; });
      const applyP = settingsMod.runAppSettingsApplyBoundary(async () => {
        enter();
        await gate.promise;
        await settingsMod.readBackAppliedAppSettings();
      });
      await entered;
      store.getState().updateSettings({ fontSize: 18 });     // A
      store.getState().updateSettings({ lineHeight: 220 });  // B
      await nTick(0);
      gate.resolve();
      await nTick(0); await nTick(0);
      checkNew('(dedup-v0) 前置：应用回读挂起（受控）', B.pendingGetSetting.length === 1);
      B.pendingGetSetting.shift().release(); // 回读：远端新值（lineHeight 200 / dedup true）
      await applyP; await nTick(0); await nTick(0); await nTick(0);
      checkNew('(dedup-v1) 反例一前置：远端键（lineHeight=200）≠旧本地（180），回读已落地、A 已发出',
        store.getState().settings.lineHeight === 220 && B.pendingSetSetting.length === 1);
      const vA = JSON.parse(B.pendingSetSetting[0].args.value);
      checkNew('(dedup-v2) A 载荷=基线+本笔 patch：lineHeight 取远端 200（不回退旧本 180）',
        vA.fontSize === 18 && vA.lineHeight === 200 && vA.smartDedup === true);

      B.pendingSetSetting[0].commit();
      B.pendingSetSetting.shift().release(); // A 成功
      const vToasts = await captureToasts(async () => { await nTick(0); await nTick(0); await nTick(0); });
      checkNew('(dedup-v3) B 失败：其键回投到基线 200，UI/DB 一致（旧本 180 不复活）',
        store.getState().settings.lineHeight === 200 && JSON.parse(B.settingsRaw).lineHeight === 200
        && store.getState().settings.fontSize === 18
        && vToasts.some((t) => String(t).includes('设置保存失败')));
    }
    {
      const { noteConfirmedAppSettings } = await import('../../dist-test/store/slices/settings.js');
      await boot([mkRow({ id: 97, feed_id: 10, url: null })], false);
      store.setState({ settings: { ...store.getState().settings, fontSize: 16, lineHeight: 180, smartDedup: false } });
      noteConfirmedAppSettings(JSON.stringify(store.getState().settings));
      B.setSettingPlans = [{ mode: 'reject' }, { mode: 'manual' }];
      store.getState().updateSettings({ fontSize: 18 });    // A（失败）
      store.getState().updateSettings({ lineHeight: 220 }); // B（成功）
      await captureToasts(async () => { await nTick(0); await nTick(0); await nTick(0); });
      checkNew('(dedup-v4) A 失败：其键回投基线 16；B 载荷不含 A 的失败值（不被偷提交）',
        store.getState().settings.fontSize === 16 && B.pendingSetSetting.length === 1
        && JSON.parse(B.pendingSetSetting[0].args.value).fontSize === 16
        && JSON.parse(B.pendingSetSetting[0].args.value).lineHeight === 220);
      B.pendingSetSetting.shift().release();
      await nTick(0); await nTick(0);
      checkNew('(dedup-v5) B 成功：UI/DB 一致（fontSize 16 / lineHeight 220）',
        store.getState().settings.lineHeight === 220 && JSON.parse(B.settingsRaw).lineHeight === 220
        && store.getState().settings.fontSize === 16);
    }

    /* ============================================================
       (W) R5/反例二（controller-postgate-unconfirmed-r4 等价）：回读失败后
           **窗口外**普通点击——基线未知必先校准；校准成功以远端为底发送、
           校准失败不发送（远端键保留、本笔回投、可见失败）。附连续失败回投。
       ============================================================ */
    {
      const settingsMod = await import('../../dist-test/store/slices/settings.js');
      await boot([mkRow({ id: 98, feed_id: 10, url: null })], false);
      store.setState({ settings: { ...store.getState().settings, fontSize: 16, lineHeight: 180, smartDedup: false } });
      const full = JSON.stringify(store.getState().settings);
      B.deferGetSettingOnce = full; // 回读（将失败）
      const applyP = settingsMod.runAppSettingsApplyBoundary(async () => {
        try { await settingsMod.readBackAppliedAppSettings(); } catch { settingsMod.markAppSettingsUnconfirmed(); }
      });
      await nTick(0); await nTick(0);
      B.pendingGetSetting.shift().reject(); // 回读失败 → 基线未知
      await applyP; await nTick(0); await nTick(0);
      checkNew('(dedup-w0) 前置：回读失败后基线未知（本地仍显示旧值）',
        store.getState().settings.smartDedup === false);
      /* 远端已应用（未被读回）：独立持久态 dedup=true。 */
      B.settingsRaw = JSON.stringify({ ...JSON.parse(full), smartDedup: true });
      B.setSettingPlan = { mode: 'manual' };
      store.getState().updateSettings({ fontSize: 18 }); // 窗口外点击
      await nTick(0); await nTick(0); await nTick(0);
      checkNew('(dedup-w1) 窗口外点击先校准：A 载荷含远端 dedup=true + 本笔 18（旧全量不覆写远端键）',
        B.pendingSetSetting.length === 1
        && JSON.parse(B.pendingSetSetting[0].args.value).smartDedup === true
        && JSON.parse(B.pendingSetSetting[0].args.value).fontSize === 18
        && B.getSettingCalls >= 2,
        JSON.stringify({ sent: B.pendingSetSetting.length, reads: B.getSettingCalls }));
      B.pendingSetSetting.shift().release();
      await nTick(0); await nTick(0);
      checkNew('(dedup-w2) 校准后发送成功：UI/DB 一致（dedup=true / fontSize=18）',
        store.getState().settings.smartDedup === true && store.getState().settings.fontSize === 18
        && JSON.parse(B.settingsRaw).smartDedup === true);
      /* 连续失败：两笔都失败 → 各自回投到基线，UI/DB 不残留 */
      B.setSettingPlans = [{ mode: 'reject' }, { mode: 'reject' }];
      const wToasts = await captureToasts(async () => {
        store.getState().updateSettings({ fontSize: 20 });
        store.getState().updateSettings({ lineHeight: 240 });
        await nTick(0); await nTick(0); await nTick(0);
      });
      checkNew('(dedup-w3) 连续失败：两笔均回投到基线（18/180），每笔失败均可见',
        store.getState().settings.fontSize === 18 && store.getState().settings.lineHeight === 180
        && wToasts.filter((t) => String(t).includes('设置保存失败')).length >= 2);
    }
    {
      const settingsMod = await import('../../dist-test/store/slices/settings.js');
      await boot([mkRow({ id: 99, feed_id: 10, url: null })], false);
      store.setState({ settings: { ...store.getState().settings, fontSize: 16, smartDedup: false } });
      const full = JSON.stringify(store.getState().settings);
      B.deferGetSettingOnce = full;
      const applyP = settingsMod.runAppSettingsApplyBoundary(async () => {
        try { await settingsMod.readBackAppliedAppSettings(); } catch { settingsMod.markAppSettingsUnconfirmed(); }
      });
      await nTick(0); await nTick(0);
      B.pendingGetSetting.shift().reject();
      await applyP; await nTick(0); await nTick(0);
      B.settingsRaw = JSON.stringify({ ...JSON.parse(full), smartDedup: true }); // 远端已应用（未读回）
      B.rejectGetSettingOnce = true; // 校准也失败
      const w2Toasts = await captureToasts(async () => {
        store.getState().updateSettings({ fontSize: 18 });
        await nTick(0); await nTick(0); await nTick(0);
      });
      checkNew('(dedup-w4) 校准失败：不发送、库中远端键保留（dedup=true）、UI 本笔键回投 16、失败可见',
        B.setSettingCalls === 0 && JSON.parse(B.settingsRaw).smartDedup === true
        && store.getState().settings.fontSize === 16
        && w2Toasts.some((t) => String(t).includes('配置基线未确认')),
        JSON.stringify({ calls: B.setSettingCalls, dedup: JSON.parse(B.settingsRaw).smartDedup, fontSize: store.getState().settings.fontSize }));
    }

    /* ============================================================
       (X) R5/反例三（controller-postgate-click-r4 等价）：门释放后 A 网络在途，
           此时 C 点击——B 的载荷不得偷带 C 的未落定意图；C 失败后 DB/UI 回基线。
           独立持久态 + 提交/回应分离（manual/commit/reject）。
       ============================================================ */
    {
      const settingsMod = await import('../../dist-test/store/slices/settings.js');
      await boot([mkRow({ id: 191, feed_id: 10, url: null })], false);
      store.setState({ settings: { ...store.getState().settings, fontSize: 16, lineHeight: 180, smartDedup: false } });
      await store.getState().bootstrapSettings(); // 完整快照为基线（读自 B.settingsRaw）
      B.setSettingPlans = [{ mode: 'manual' }, { mode: 'manual' }, { mode: 'reject' }];
      const gate = mkDeferred();
      let enter; const entered = new Promise((r) => { enter = r; });
      const applyP = settingsMod.runAppSettingsApplyBoundary(async () => {
        enter();
        await gate.promise;
        await settingsMod.readBackAppliedAppSettings();
      });
      await entered;
      store.getState().updateSettings({ fontSize: 18 });     // A（门内）
      store.getState().updateSettings({ lineHeight: 200 });  // B（门内）
      await nTick(0);
      gate.resolve();
      await applyP; await nTick(0); await nTick(0); await nTick(0);
      checkNew('(dedup-x1) 前置：A 已发出并挂起（manual），B/C 未发',
        B.pendingSetSetting.length === 1 && JSON.parse(B.pendingSetSetting[0].args.value).fontSize === 18);
      store.getState().updateSettings({ smartDedup: true }); // C（门释放后、A 在途）
      await nTick(0); await nTick(0);
      checkNew('(dedup-x2) 前置：C 排队（A 未结算，B/C 均未发出）', B.pendingSetSetting.length === 1);
      B.pendingSetSetting[0].commit();
      B.pendingSetSetting.shift().release(); // A 成功
      await nTick(0); await nTick(0); await nTick(0);
      const xB = B.pendingSetSetting.length === 1 ? JSON.parse(B.pendingSetSetting[0].args.value) : {};
      checkNew('(dedup-x3) B 载荷不含 C 的未落定 dedup=true（仍 false），含 A 的 18 与自身 200',
        B.pendingSetSetting.length === 1 && xB.smartDedup === false
        && xB.fontSize === 18 && xB.lineHeight === 200,
        JSON.stringify({ sent: B.pendingSetSetting.length, dedup: xB.smartDedup }));
      B.pendingSetSetting[0].commit();
      B.pendingSetSetting.shift().release(); // B 成功
      const xToasts = await captureToasts(async () => { await nTick(0); await nTick(0); await nTick(0); });
      checkNew('(dedup-x4) C 失败：不落库、UI/DB dedup=false（B 未偷带、C 不残留）',
        store.getState().settings.smartDedup === false && JSON.parse(B.settingsRaw).smartDedup === false
        && xToasts.some((t) => String(t).includes('设置保存失败')));
    }

    /* ============================================================
       (Y) R5：**校准在途期间的点击**——其意图不被校准合并吞掉，也不被前一笔
           偷带；校准成功序 A→B 各自按「基线+自身 patch」发送。
       ============================================================ */
    {
      await boot([mkRow({ id: 192, feed_id: 10, url: null })], false);
      store.setState({ settings: { ...store.getState().settings, fontSize: 16, themeMode: 'dark', smartDedup: false } });
      B.setSettingPlans = [{ mode: 'manual' }, { mode: 'manual' }];
      B.deferGetSettingOnce = JSON.stringify({ ...store.getState().settings, smartDedup: true }); // 校准读（远端）
      store.getState().updateSettings({ fontSize: 18 }); // A：基线未知 → 校准（挂起）
      await nTick(0); await nTick(0);
      checkNew('(dedup-y1) 前置：校准读取挂起（A 未发出）',
        B.pendingGetSetting.length === 1 && B.pendingSetSetting.length === 0);
      store.getState().updateSettings({ themeMode: 'light' }); // B：校准等待期间点击
      await nTick(0);
      B.pendingGetSetting.shift().release(); // 校准完成（远端 dedup=true）
      await nTick(0); await nTick(0); await nTick(0);
      const yA = B.pendingSetSetting.length === 1 ? JSON.parse(B.pendingSetSetting[0].args.value) : {};
      checkNew('(dedup-y2) A 以校准基线发送：dedup=true、fontSize=18，且不含 B 的 themeMode',
        B.pendingSetSetting.length === 1 && yA.smartDedup === true && yA.fontSize === 18 && yA.themeMode !== 'light',
        JSON.stringify({ sent: B.pendingSetSetting.length, themeMode: yA.themeMode }));
      B.pendingSetSetting[0].commit();
      B.pendingSetSetting.shift().release(); // A 成功
      await nTick(0); await nTick(0); await nTick(0);
      checkNew('(dedup-y3) B 随后发送自身意图（themeMode=light），不含 A 之外的多余键',
        B.pendingSetSetting.length === 1
        && JSON.parse(B.pendingSetSetting[0].args.value).themeMode === 'light'
        && JSON.parse(B.pendingSetSetting[0].args.value).fontSize === 18);
      B.pendingSetSetting.shift().release();
      await nTick(0); await nTick(0);
      checkNew('(dedup-y4) 落定后 UI/DB 一致：dedup=true / fontSize=18 / themeMode=light',
        store.getState().settings.smartDedup === true && store.getState().settings.fontSize === 18
        && store.getState().settings.themeMode === 'light'
        && JSON.parse(B.settingsRaw).themeMode === 'light');
    }
    {
      await boot([mkRow({ id: 193, feed_id: 10, url: null })], false);
      store.setState({ settings: { ...store.getState().settings, fontSize: 16, themeMode: 'dark', smartDedup: false } });
      const zFull = JSON.stringify(store.getState().settings); // 点击前的独立基线快照（不含乐观值）
      B.setSettingPlan = { mode: 'manual' };
      B.deferGetSettingOnce = zFull;
      store.getState().updateSettings({ fontSize: 18 }); // A：校准（将失败）
      await nTick(0); await nTick(0);
      store.getState().updateSettings({ themeMode: 'light' }); // B：校准等待期间点击
      await nTick(0);
      B.deferGetSettingOnce = JSON.stringify({ ...JSON.parse(zFull), smartDedup: true }); // B 的校准读（下一个 get_setting）
      B.pendingGetSetting.shift().reject(); // A 的校准失败
      const zToasts = await captureToasts(async () => { await nTick(0); await nTick(0); await nTick(0); });
      checkNew('(dedup-z1) A 校准失败：不发送、fontSize 回投 16、失败可见；B 校准已接手（未发出）',
        B.pendingSetSetting.length === 0 && store.getState().settings.fontSize === 16
        && zToasts.some((t) => String(t).includes('配置基线未确认')),
        JSON.stringify({ sent: B.pendingSetSetting.length, fontSize: store.getState().settings.fontSize }));
      checkNew('(dedup-z1b) 前置：B 的校准读取已挂起', B.pendingGetSetting.length === 1);
      B.pendingGetSetting.shift().release(); // B 的校准完成（远端 dedup=true）
      await nTick(0); await nTick(0); await nTick(0);
      const zB = B.pendingSetSetting.length === 1 ? JSON.parse(B.pendingSetSetting[0].args.value) : {};
      checkNew('(dedup-z2) B 自身校准成功：以基线发送 themeMode=light（不含 A 的失败 font18，retain dedup=true）',
        B.pendingSetSetting.length === 1 && zB.themeMode === 'light' && zB.smartDedup === true && zB.fontSize === 16,
        JSON.stringify({ sent: B.pendingSetSetting.length, fontSize: zB.fontSize }));
      B.pendingSetSetting.shift().release();
      await nTick(0); await nTick(0);
      checkNew('(dedup-z3) B 成功：UI/DB 一致（themeMode=light / fontSize=16 / dedup=true）',
        store.getState().settings.themeMode === 'light' && store.getState().settings.fontSize === 16
        && store.getState().settings.smartDedup === true
        && JSON.parse(B.settingsRaw).themeMode === 'light');
    }

    /* ============================================================
       (AA) R6/反例一（controller-empty-base-r5 等价）：空 / partial / 未知基线
           的同键连续失败——失败意图不得经后笔 prev 残留（稳定底值下传）。
       ============================================================ */
    {
      const settingsMod = await import('../../dist-test/store/slices/settings.js');
      // AA1：空库（getSetting=null）→ 已知空基线
      await boot([mkRow({ id: 194, feed_id: 10, url: null })], false);
      store.setState({ settings: { ...store.getState().settings, fontSize: 16 } });
      B.settingsRaw = null; // 全新库：无存量设置
      await store.getState().bootstrapSettings(); // 已知空基线
      B.setSettingPlans = [{ mode: 'reject' }, { mode: 'reject' }];
      const aaToasts = await captureToasts(async () => {
        store.getState().updateSettings({ fontSize: 18 });
        store.getState().updateSettings({ fontSize: 20 });
        await nTick(0); await nTick(0); await nTick(0);
      });
      checkNew('(dedup-aa1) 空库同键连续失败：UI 回稳定底值 16（不残留 18）、两笔失败均可见',
        store.getState().settings.fontSize === 16
        && aaToasts.filter((t) => String(t).includes('设置保存失败')).length >= 2,
        JSON.stringify({ font: store.getState().settings.fontSize, toasts: aaToasts.length }));
      // AA2：partial 基线（原文不含 fontSize 键）
      await boot([mkRow({ id: 195, feed_id: 10, url: null })], false);
      store.setState({ settings: { ...store.getState().settings, fontSize: 16 } });
      B.settingsRaw = JSON.stringify({ smartDedup: false });
      await store.getState().bootstrapSettings();
      B.setSettingPlans = [{ mode: 'reject' }, { mode: 'reject' }];
      const aa2Toasts = await captureToasts(async () => {
        store.getState().updateSettings({ fontSize: 18 });
        store.getState().updateSettings({ fontSize: 20 });
        await nTick(0); await nTick(0); await nTick(0);
      });
      checkNew('(dedup-aa2) partial 基线同键连续失败：UI 回 16（无键 → 稳定底值链）',
        store.getState().settings.fontSize === 16
        && aa2Toasts.filter((t) => String(t).includes('设置保存失败')).length >= 2,
        JSON.stringify({ font: store.getState().settings.fontSize }));
      // AA3：未知基线（回读失败）＋校准连续失败
      await boot([mkRow({ id: 196, feed_id: 10, url: null })], false);
      store.setState({ settings: { ...store.getState().settings, fontSize: 16 } });
      const full196 = JSON.stringify(store.getState().settings);
      B.deferGetSettingOnce = full196;
      const aaApply = settingsMod.runAppSettingsApplyBoundary(async () => {
        try { await settingsMod.readBackAppliedAppSettings(); } catch { settingsMod.markAppSettingsUnconfirmed(); }
      });
      await nTick(0); await nTick(0);
      B.pendingGetSetting.shift().reject(); // 回读失败 → 基线未知
      await aaApply; await nTick(0); await nTick(0);
      B.rejectGetSettingCount = 2; // 两笔的校准都失败
      const aa3Toasts = await captureToasts(async () => {
        store.getState().updateSettings({ fontSize: 18 });
        store.getState().updateSettings({ fontSize: 20 });
        await nTick(0); await nTick(0); await nTick(0);
      });
      checkNew('(dedup-aa3) 未知基线＋校准连续失败：两笔都不发送、UI 回 16、失败可见',
        B.setSettingCalls === 0 && store.getState().settings.fontSize === 16
        && aa3Toasts.some((t) => String(t).includes('配置基线未确认')),
        JSON.stringify({ calls: B.setSettingCalls, font: store.getState().settings.fontSize }));
    }

    /* ============================================================
       (AB) R6/反例二（controller-shared-old-read-r5 等价）：跨提交不得 join
           旧共享读取为新鲜——写成功 / 写失败两个方向；真正新鲜外部读可接受。
       ============================================================ */
    {
      const settingsMod = await import('../../dist-test/store/slices/settings.js');
      // AB1：bootstrap1 旧读挂起 → 本地写 true 成功 → bootstrap2 强制新鲜读
      await boot([mkRow({ id: 197, feed_id: 10, url: null })], false);
      store.setState({ settings: { ...store.getState().settings, smartDedup: false } });
      B.settingsRaw = JSON.stringify({ ...store.getState().settings, smartDedup: false });
      settingsMod.noteConfirmedAppSettings(B.settingsRaw);
      B.deferGetSettingOnce = B.settingsRaw; // bootstrap1 的旧读（挂起）
      const first = store.getState().bootstrapSettings();
      await nTick(0);
      checkNew('(dedup-ab1) 前置：bootstrap1 旧读挂起', B.pendingGetSetting.length === 1);
      store.getState().updateSettings({ smartDedup: true }); // 本地写成功（基线推进）
      await settingsMod.awaitAppSettingsWriteFrontier();
      const readsBeforeSecond = B.getSettingCalls;
      const second = store.getState().bootstrapSettings(); // 不得 join 旧读
      await nTick(0); await nTick(0); await nTick(0);
      checkNew('(dedup-ab2) bootstrap2 不 join 跨提交旧读：强制发起新鲜读取（reads 增 1）',
        B.getSettingCalls === readsBeforeSecond + 1,
        JSON.stringify({ before: readsBeforeSecond, after: B.getSettingCalls }));
      B.pendingGetSetting.shift().release(); // 放行旧读（false）
      await Promise.all([first, second]);
      await nTick(0); await nTick(0);
      checkNew('(dedup-ab3) 旧读放行不得倒灌：先写成功的 UI/DB 一致 true',
        store.getState().settings.smartDedup === true && JSON.parse(B.settingsRaw).smartDedup === true);
    }
    {
      const settingsMod = await import('../../dist-test/store/slices/settings.js');
      // AB2：跨失败——bootstrap 旧读挂起、写失败后再 bootstrap：同样强制新鲜读
      await boot([mkRow({ id: 198, feed_id: 10, url: null })], false);
      store.setState({ settings: { ...store.getState().settings, smartDedup: false } });
      B.settingsRaw = JSON.stringify({ ...store.getState().settings, smartDedup: false });
      settingsMod.noteConfirmedAppSettings(B.settingsRaw);
      B.deferGetSettingOnce = B.settingsRaw;
      const first = store.getState().bootstrapSettings();
      await nTick(0);
      B.setSettingPlan = { mode: 'reject' };
      store.getState().updateSettings({ smartDedup: true }); // 写失败（回投 false）
      await captureToasts(async () => { await settingsMod.awaitAppSettingsWriteFrontier(); await nTick(0); });
      const readsBefore = B.getSettingCalls;
      const second = store.getState().bootstrapSettings(); // 跨失败同样不得 join
      await nTick(0); await nTick(0); await nTick(0);
      checkNew('(dedup-ab4) 跨失败后 bootstrap 同样强制新鲜读（不把旧读当新读）',
        B.getSettingCalls === readsBefore + 1 && store.getState().settings.smartDedup === false);
      B.pendingGetSetting.shift().release();
      await Promise.all([first, second]);
      await nTick(0);
      checkNew('(dedup-ab5) 真正新鲜外部读可接受：bootstrap 读回 false（与库一致）',
        store.getState().settings.smartDedup === false);
    }
  } finally {
    restoreInvoke();
  }
}
