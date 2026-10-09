// tools/frontend-tests/scroll-position-restore.mjs
// 领域模块：滚动保位/切换返回/阅读器关闭焦点归还（timelineAnchor 全链路）
// OPT-016C 拆分自 tools/frontend-regression.mjs（旧行区间 4220-4475、4477-4691、4693-4979）；
// 断言名称/条件文本原样迁移，仅做路径深度适配（import.meta.url 与动态 import 深一层）与
// 共享可变状态的 S. 归属重写。数据所有权：共享假后端/夹具/记录归 harness（见 harness.mjs 头注），
// 本模块不 import 第二份 store；域内自带夹具（大行集等）仍在本模块内独立构造与复位。
export const id = 'scroll-position-restore';

export async function run(ctx) {
  const { S, store, checkNew, nTick, resetStore, NOW, iso, mkRow, queryRows, selectVisibleEntries, viewEntriesCache } = ctx;
  await ctx.useMainBackend();

  /* ============================================================
     TASK-111（二阶段③）：①查询缓存实体预算 ②后台刷新保位。
     ①审计：「单纯限制为 8 个视图并不能限制每个视图的大小」——TASK-100 P3-7
       只封键数（LRU 8），TASK-110 分页化后单键 entries 仍随滚动加载无界增长
       （syncCurrentViewCache 会把当前 1500 条窗口整体落键）。新契约：
       单键实体预算 VIEW_ENTRIES_CACHE_ENTRY_BUDGET=1000（≥ 页大小 500，截断
       只发生在加载超 2 页的大库场景），超限尾部截断（缓存语义=首屏快照，
       丢最老端由恢复后的后台 reload 补齐）；缓存值升级为 {entries, cursor,
       exhausted} 元数据（TASK-117：cursor 为 keyset 游标，loaded 字段承接原
       loadedCount 语义）——恢复路径（nav 三处）用**记录值**判定 exhausted 与
       续拉游标，不重算截断后长度（判别：截断长度 1000 落在页边界上方，
       「已到底」的快照会被误判成「还有数据」，且续拉锚回退重拉已去重
       丢弃的区间）。
     ②审计：「后台刷新保留当前阅读位置」——feeds-updated/手动同步/单源刷新
       → reloadFromBackend 整体替换 entries，newest_first 下新文章插头部使
       索引后移、虚拟列表视觉跳动。新契约：Timeline 滚动时节流记录顶条锚
       （可见首条目 id + filterKey，模块级 timelineAnchor），内容刷新落地时
       bump positionRestoreNonce，Timeline 消费锚：锚 id 仍在同上下文
       （filterKey 逐字一致）新快照中 → 程序性滚动回其新索引（复用既有
       suppressNextScrollEvents 抑制）；无锚/上下文已切/锚丢失 → 回落现状。
       导航路径（selectFeed/selectView/selectLayout/toggleTimelineSort）不
       bump 信号且切上下文即弃锚——分流显式。
     夹具：1501/1100 行 feed-10 大夹具驱动预算；缓存写入经 flipEntryFlag
     （→syncCurrentViewCache，loadMore 不落缓存——缓存语义=首屏/同步快照，
     既有口径）。保位用纯函数 anchorRestoreIndex + nonce 断言（滚动本体是
     DOM 行为，由源码形态断言防回退，与 t100/p3 组先例同口径）。
     ============================================================ */
  {
    const fs111 = await import('node:fs');
    const src111 = (p) => fs111.readFileSync(new URL(p, import.meta.url), 'utf8');
    const { VIEW_ENTRIES_CACHE_ENTRY_BUDGET, ARTICLES_PAGE_SIZE: t111Page, flipEntryFlag } = await import('../../dist-test/store/internals.js');
    const ta111 = await import('../../src/components/timelineAnchor.ts');
    const filterKeyOf = (s) => `${s.activeContentLayout}|${s.activeViewFilter}|${s.activeFeedFilter}|${s.timelineFilter}|${s.timelineSort}`;
    const mkT111Row = (o) => mkRow({ feed_id: 10, ...o });
    const t111Rows = (count, startId) => {
      const rows = [];
      for (let i = 0; i < count; i += 1) rows.push(mkT111Row({ id: startId + i, published_at: iso(NOW - i * 60000) }));
      return rows;
    };
    /* 缓存写入驱动器：直接翻旗（同步、无 IPC）→ flipEntryFlag 内部
       syncCurrentViewCache 把当前窗口（含全部已加载页）落键 */
    const t111SyncCache = (id) => flipEntryFlag(id, 'isStarred');

    /* -- (t111-0) 预算单点 + 审计注释 + 恢复用记录值（源级防回退） -- */
    const internalsSrc111 = src111('../../src/store/internals.ts');
    const navSrc111 = src111('../../src/store/slices/nav.ts');
    checkNew('(t111-0) 单键实体预算收口 internals：=1000 且 ≥ 页大小（截断只发生在加载超 2 页场景，首批/次页永不截断）',
      VIEW_ENTRIES_CACHE_ENTRY_BUDGET === 1000 && VIEW_ENTRIES_CACHE_ENTRY_BUDGET >= t111Page);
    /* 【TASK-117 改动理由】源级断言随游标契约更新：缓存元数据 loadedCount 升为
       完整 keyset 游标（cursor），nav 恢复点透传 cached.cursor——记录值恢复的
       保护意图不变（不用截断长度重算游标）。 */
    checkNew('(t111-0) 预算注释载明审计依据（键数上限≠单键大小），恢复路径用缓存记录的 cursor/exhausted 而非重算截断长度',
      internalsSrc111.includes('审计：「单纯限制为 8 个视图并不能限制每个视图的大小」')
      && internalsSrc111.includes('export function setViewEntriesSnapshot(')
      && navSrc111.includes('applyArticlesCursor(scopeKey, cached.cursor, cached.exhausted)')
      && navSrc111.includes('articlesExhausted: cached.exhausted'));

    /* ---------- (t111-1) 预算截断：边界（恰=预算不截断）+ 触发（尾部截断、元数据真值） ---------- */
    await resetStore();
    S.backendRows = t111Rows(1501, 3000); // 1501 行：3 页后余 1 行
    await store.getState().bootstrapFromBackend(); // 首批 500（3000..3499）
    await store.getState().loadMoreArticles(); // 1000（..3999）
    t111SyncCache('3000'); // 缓存落键：当前窗口恰 1000 = 预算
    const t111c1000 = viewEntriesCache.get('article|all|all');
    /* 【TASK-117 改动理由】缓存元数据断言随游标契约更新：loadedCount → cursor.loaded */
    checkNew('(t111-1) 边界：加载恰 1000（=预算）不截断，缓存值=完整快照 + 元数据（cursor.loaded=1000、exhausted=false）',
      t111c1000.entries.length === 1000 && t111c1000.cursor.loaded === 1000 && t111c1000.exhausted === false
      && t111c1000.entries[0].id === '3000' && t111c1000.entries.at(-1).id === '3999');
    await store.getState().loadMoreArticles(); // 1500（..4499）→ 超预算
    t111SyncCache('3002');
    const t111c1500 = viewEntriesCache.get('article|all|all');
    checkNew('(t111-1) 截断触发（1500>1000）：尾部截断为 1000 条（首条保留、最老端 500 条丢弃——首屏快照语义，恢复后由后台刷新补齐），cursor/exhausted 记录写入时真值（loaded=1500、锚=4499/false）不随截断失真',
      t111c1500.entries.length === 1000 && t111c1500.entries[0].id === '3000' && t111c1500.entries.at(-1).id === '3999'
      && t111c1500.cursor.loaded === 1500 && t111c1500.cursor.lastId === 4499 && t111c1500.exhausted === false);

    /* ---------- (t111-2) 截断恢复（exhausted=false 态）+ 截断后续拉衔接（判别） ----------
       离开/回到该范围触发缓存恢复；注入 reload 失败让恢复态存续（否则后台
       刷新立即整体替换）。判别点：恢复游标 = 记录的 cursor.loaded=1500（朴素
       实现重算截断长度会写 1000），续拉 offset=1500 不回退重拉。 */
    S.listPlan = { mode: 'reject', error: { message: 't111 注入' } };
    store.getState().selectFeed('feed-11'); // 离开（其后台 reload 失败被吞）
    await nTick(10);
    store.getState().selectFeed('all'); // 回来：缓存命中同步恢复
    const t111rA = store.getState();
    checkNew('(t111-2) 截断快照恢复（exhausted=false 态）：entries=截断后的 1000 条、游标按记录 cursor.loaded=1500 恢复（判别：重算截断长度会写 1000）、exhausted=false 与写入时一致（末次拉取满页）',
      t111rA.entries.length === 1000 && t111rA.entries[0].id === '3000'
      && t111rA.articlesCursor['article|all']?.loaded === 1500 && t111rA.articlesLimit === 1500
      && t111rA.articlesExhausted === false);
    await nTick(10); // 让恢复路径的注入失败 reload 落定（backendReloadInFlight 归零，续拉不再被在途守卫拦截）
    S.listPlan = null; // 解除 reject 注入：续拉本身要走通（只用于冻结恢复路径的 reload）
    S.invokeCalls.length = 0;
    await store.getState().loadMoreArticles(); // 续拉：keyset 锚必须接在真实已加载窗口末行之后
    const t111contArgs = S.invokeCalls.find((c) => c.cmd === 'list_articles')?.args.args;
    const t111rA2 = store.getState();
    /* 【TASK-117 改动理由】续拉 wire 从 offset=1500 改为 keyset 锚（恢复游标记录的
       锚=第三页末行 4499）——判别意图不变：从记录的真值续拉，不回退重拉已丢弃区间。 */
    checkNew('(t111-2) 截断后续拉衔接（判别）：keyset 锚用恢复游标记录的末行 4499/loaded=1500（朴素实现会从截断长度 1000 重拉已去重丢弃的区间）、余下 1 行（4500）追加无重复、游标=1501、exhausted=true（1<500）',
      t111contArgs?.last_id === 4499 && t111contArgs?.last_published === iso(NOW - 1499 * 60000)
      && t111contArgs?.offset === undefined && t111contArgs?.limit === 500
      && t111rA2.entries.length === 1001 && t111rA2.entries.at(-1)?.id === '4500'
      && new Set(t111rA2.entries.map((e) => e.id)).size === 1001
      && t111rA2.articlesCursor['article|all']?.loaded === 1501 && t111rA2.articlesExhausted === true);

    /* ---------- (t111-3) 截断恢复（exhausted=true 态，判别）+ exhausted 不误判 ----------
       1100 行 = 500+500+100：末页不满 ⇒ 真实已到底。截断后缓存长度 1000 落在
       页边界上方——重算长度会把「已到底」误判成「还有数据」并触发伪续拉。 */
    await resetStore();
    S.backendRows = t111Rows(1100, 5000); // 1100 行（5000..6099）
    await store.getState().bootstrapFromBackend(); // 500
    await store.getState().loadMoreArticles(); // 1000
    await store.getState().loadMoreArticles(); // 余 100 行（100<500）→ 真实到底
    t111SyncCache('5001'); // 缓存落键：1100 条 → 截断 1000，exhausted=true
    const t111cB = viewEntriesCache.get('article|all|all');
    checkNew('(t111-3) 前置：加载 1100（末页不满）→ 缓存截断 1000 条但元数据记录 cursor.loaded=1100 / exhausted=true（尾部截断不丢 exhausted 判据）',
      t111cB.entries.length === 1000 && t111cB.cursor.loaded === 1100 && t111cB.exhausted === true);
    S.listPlan = { mode: 'reject', error: { message: 't111 注入' } };
    store.getState().selectFeed('feed-11');
    await nTick(10);
    store.getState().selectFeed('all');
    const t111rB = store.getState();
    checkNew('(t111-3) 截断恢复（exhausted=true 态，判别）：恢复 exhausted=true（判别：重算截断长度 1000≥500 会误判成 false）、游标=记录 cursor.loaded=1100',
      t111rB.entries.length === 1000 && t111rB.articlesExhausted === true
      && t111rB.articlesCursor['article|all']?.loaded === 1100);
    await nTick(10); // 让恢复路径的注入失败 reload 落定（同 t111-2）
    S.invokeCalls.length = 0;
    await store.getState().loadMoreArticles();
    checkNew('(t111-3) exhausted 不误判：恢复后的「已到底」状态拦截伪续拉（不发 list_articles，列表不重复入列）',
      S.invokeCalls.filter((c) => c.cmd === 'list_articles').length === 0
      && store.getState().entries.length === 1000);
    S.listPlan = null;

    /* ---------- (t111-4) 后台刷新保位：节流锚记录 + 落地信号 + 锚回位决策 ----------
       新文章插头部场景：锚 id 7001 从 index 0 后移到 3，决策返回新索引 →
       Timeline 程序性滚动回位（视觉跳动抵消）；导航路径不 bump 信号。 */
    await resetStore();
    ta111.clearTopAnchor();
    S.backendRows = [
      mkT111Row({ id: 7001, published_at: iso(NOW) }),
      mkT111Row({ id: 7002, published_at: iso(NOW - 1000) }),
      mkT111Row({ id: 7003, published_at: iso(NOW - 2000) }),
    ];
    await store.getState().bootstrapFromBackend();
    const t111nonce0 = store.getState().positionRestoreNonce;
    const t111fkey = filterKeyOf(store.getState());
    const t111idsBefore = selectVisibleEntries(store.getState()).map((e) => e.id);
    const t111T0 = NOW + 10000000;
    checkNew('(t111-4) 顶条锚节流记录：首次记录生效、窗口内重复记录被忽略（约 4 次/秒收敛，锚零订阅不触发渲染）',
      t111idsBefore.join(',') === '7001,7002,7003'
      && ta111.recordTopAnchor(t111idsBefore[0], t111fkey, t111T0) === true
      && ta111.recordTopAnchor('7003', t111fkey, t111T0 + 100) === false
      && ta111.peekTopAnchor()?.id === '7001');
    /* 后台刷新：3 篇新文章插入头部（时间戳更新） */
    S.backendRows = [
      mkT111Row({ id: 7102, published_at: iso(NOW + 2000) }),
      mkT111Row({ id: 7101, published_at: iso(NOW + 1000) }),
      mkT111Row({ id: 7100, published_at: iso(NOW) }),
      ...S.backendRows,
    ];
    await store.getState().reloadFromBackend({ keepReadingPosition: true });
    const t111st4 = store.getState();
    checkNew('(t111-4) 后台刷新（keepReadingPosition）落地：保位信号 nonce +1（当前视图 all，落地即发出）',
      t111st4.positionRestoreNonce === t111nonce0 + 1 && t111st4.entries.length === 6);
    const t111items4 = selectVisibleEntries(t111st4);
    checkNew('(t111-4) 顶条锚回位决策（新文章插头部场景）：锚 id 7001 由 index 0 后移到 3，决策返回新索引（Timeline 据此 scrollToIndex 原位还原）',
      t111items4.map((e) => e.id).join(',') === '7102,7101,7100,7001,7002,7003'
      && ta111.anchorRestoreIndex(ta111.peekTopAnchor(), filterKeyOf(t111st4), t111items4) === 3);
    const t111nonce4b = store.getState().positionRestoreNonce;
    await store.getState().reloadFromBackend(); // 导航路径形态：不带 opts
    checkNew('(t111-4) 分流（导航路径）：不带 keepReadingPosition 的 reload 不 bump 保位信号——用户主动导航时锚机制保持沉默',
      store.getState().positionRestoreNonce === t111nonce4b);
    /* 筛选视图：selectView 导航不 bump；后台刷新经 reloadFilteredEntries 透传落地 bump。
       【TASK-111 R1/F1 改动理由】原断言把 nonce 基线放在 selectView 落定**之后**读取
       再与自身比较（永不失败，审查变异 M3：selectView 无条件 bump 全量仍绿——空洞）。
       改为真前后比较：基线在调用前读取，selectView 落定后必须仍等于基线。 */
    const t111nonce4c0 = store.getState().positionRestoreNonce; // 基线：selectView 调用前
    store.getState().selectView('starred'); // 无收藏行 → 空列表（导航路径）
    await nTick(20);
    checkNew('(t111-4) 分流（筛选视图导航）：selectView 不 bump 保位信号（真前后比较：调用前基线 vs 落定后——selectView 任何形式的 bump 都会转红）',
      store.getState().positionRestoreNonce === t111nonce4c0);
    const t111nonce4c = store.getState().positionRestoreNonce; // 透传 bump 用例的基线（selectView 落定后）
    S.backendRows.unshift(mkT111Row({ id: 7200, is_starred: true, published_at: iso(NOW + 3000) }));
    await store.getState().reloadFilteredEntries('starred', { keepReadingPosition: true });
    checkNew('(t111-4) 分流（筛选视图后台刷新）：reloadFilteredEntries 透传 keepReadingPosition 落地 bump 信号（feeds-updated → 筛选视图透传路径；落地快照即当前视图）',
      store.getState().positionRestoreNonce === t111nonce4c + 1
      && store.getState().entries.map((e) => e.id).join(',') === '7200');

    /* ---------- (t111-5) 锚丢失回落 / 主动切范围回落 / 锚不干扰主动滚动 ---------- */
    /* a) 锚丢失：锚 id 不在新快照（文章被删/被筛出）→ 决策 null，回落现状 */
    ta111.clearTopAnchor();
    const t111fkeyS = filterKeyOf(store.getState()); // starred 上下文
    ta111.recordTopAnchor('7200', t111fkeyS, NOW + 20000000);
    S.backendRows = S.backendRows.filter((r) => r.id !== 7200); // 后端：该收藏文消失
    const t111nonce5a = store.getState().positionRestoreNonce;
    await store.getState().reloadFilteredEntries('starred', { keepReadingPosition: true });
    checkNew('(t111-5) 锚丢失回落（判别）：锚 id 不在新快照 → 决策返回 null（回落现状、不强制顶部——消费侧无可滚动目标），信号照常送达（nonce +1，消费侧对 null 不动作）',
      ta111.anchorRestoreIndex(ta111.peekTopAnchor(), filterKeyOf(store.getState()), selectVisibleEntries(store.getState())) === null
      && store.getState().positionRestoreNonce === t111nonce5a + 1);
    /* b) 主动切范围：锚属于旧上下文（filterKey 失配）→ 不回位。
       【TASK-111 R1/F2 改动理由】原场景切范围后可见列表为空（唯一收藏行已删），
       anchorRestoreIndex 返回 null 只因 findIndex 落空、与 filterKey 拦截无关
       （审查变异 M2：删除 timelineAnchor 的 filterKey 拦截全量仍绿——判别力不成立）。
       改为「同源换范围」构造：切换后可见集合与切换前**完全一致**（全部行都属于
       feed-10，锚 id 仍是可见首条），唯一变化是 filterKey 的范围段——决策返回
       null 只能来自 filterKey 拦截本身（M2 变异下该断言必转红）。 */
    await resetStore(); // 独立夹具：all 范围两行（均属 feed-10）
    ta111.clearTopAnchor();
    S.backendRows = [
      mkT111Row({ id: 7300, published_at: iso(NOW) }),
      mkT111Row({ id: 7301, published_at: iso(NOW - 1000) }),
    ];
    await store.getState().bootstrapFromBackend(); // 范围 all：可见 [7300, 7301]
    const t111fkey5b = filterKeyOf(store.getState()); // 旧上下文（范围 all）
    ta111.recordTopAnchor('7300', t111fkey5b, NOW + 21000000);
    const t111nonce5b = store.getState().positionRestoreNonce;
    store.getState().selectFeed('10'); // 用户主动切范围（真实 scope 形态 '10'：feedId 无前缀；导航，不 bump 信号）
    await nTick(20);
    const t111st5b = store.getState();
    const t111items5b = selectVisibleEntries(t111st5b);
    const t111idx5b = t111items5b.findIndex((e) => e.id === '7300');
    checkNew('(t111-5) 主动切范围回落（判别）：filterKey 已失配且锚 id 仍可见（可见列表非空、findIndex 命中——排除「findIndex 落空」假阳性）→ 决策 null 纯因 filterKey 拦截（绝不把上一个范围的阅读位置回放进新范围），导航 reload 不 bump 信号',
      filterKeyOf(t111st5b) !== t111fkey5b
      && t111items5b.length === 2 && t111idx5b === 0
      && ta111.anchorRestoreIndex(ta111.peekTopAnchor(), filterKeyOf(t111st5b), t111items5b) === null
      && t111st5b.positionRestoreNonce === t111nonce5b);
    /* c) 主动滚动：锚跟随用户——节流窗口过期后可重新记录（边界 249/250） */
    const t111T5 = NOW + 30000000;
    checkNew('(t111-5) 锚不干扰主动滚动：节流窗口过期后重新记录生效（锚跟随用户最终停留位置）、窗口内仍被忽略',
      ta111.recordTopAnchor('7101', filterKeyOf(store.getState()), t111T5) === true
      && ta111.recordTopAnchor('7102', filterKeyOf(store.getState()), t111T5 + 249) === false
      && ta111.recordTopAnchor('7102', filterKeyOf(store.getState()), t111T5 + 250) === true
      && ta111.peekTopAnchor()?.id === '7102');

    /* -- (t111-6) Timeline / 内容刷新入口接线（源级防回退，与 t109 同口径） -- */
    const timelineSrc111 = src111('../../src/components/Timeline.tsx');
    const appSrc111 = src111('../../src/App.tsx');
    const syncSrc111 = src111('../../src/store/slices/sync.ts');
    const feedsSrc111 = src111('../../src/store/slices/feeds.ts');
    const t111iRestore = timelineSrc111.indexOf('anchorRestoreIndex(peekTopAnchor(), filterKey, items)');
    const t111iSuppress = timelineSrc111.indexOf('suppressNextScrollEvents();', t111iRestore);
    const t111iScroll = timelineSrc111.indexOf("rowVirtualizer.scrollToIndex(idx, { align: 'start' });", t111iSuppress);
    /* 【TASK-123 改动理由】recordTopAnchor 调用补第 4 参（卡片内像素偏移，审计
       P2-5③ 锚载荷扩展）：保护意图不变（handleScroll 顶条锚记录接线），字面量
       随调用形态同步更新——偏移测量收口在 measureAnchorOffsetPx 单点。 */
    checkNew('(t111-6) Timeline 接线：滚动记录顶条锚（recordTopAnchor）、上下文切换弃锚（clearTopAnchor）、保位信号消费——先程序性滚动抑制再 scrollToIndex 回位（复用既有机制，防回位被误判为用户滚动）',
      timelineSrc111.includes('const positionRestoreNonce = useAppStore((s) => s.positionRestoreNonce);')
      && timelineSrc111.includes('recordTopAnchor(topItem.id, filterKey, performance.now(), measureAnchorOffsetPx(topIndex))')
      && timelineSrc111.includes('clearTopAnchor();')
      && t111iRestore >= 0 && t111iSuppress > t111iRestore && t111iScroll > t111iSuppress
      && t111iScroll - t111iSuppress < 120);
    checkNew('(t111-6) 内容刷新入口携带保位请求（feeds-updated / 手动同步 / 单源刷新），导航入口（nav 三处 + 启动装载）保持无参调用',
      appSrc111.includes('reloadFromBackend({ keepReadingPosition: true })')
      && syncSrc111.includes('reloadFromBackend({ keepReadingPosition: true })')
      && feedsSrc111.includes('reloadFromBackend({ keepReadingPosition: true })'));

    await resetStore(); // 夹具复位：不把大夹具与 reject 注入带给后续块
  }

  /* ============================================================
     TASK-115（2026-10-06，REQ-005 三阶段②）：返回位置统一规则。
     X1 切换返回滚动恢复（per-filterKey 锚存档 + nav 缓存命中恢复信号）/
     X2 阅读器关闭焦点归还原卡（关闭信号 + ref 记账）/ X3 规则文档化
     （timelineAnchor.ts 头注 = 全场景规则表的代码单点注释）。

     与 TASK-111 的证据边界同口径（如实说明）：滚动/聚焦本体是 DOM 行为，
     node 回归网无法驱动真实虚拟列表——行为断言落在两层：
     - store 层：信号 bump 语义（导航缓存命中 → switchRestoreNonce；阅读器
       关闭 → readerCloseNonce）与「不叠加」双向隔离（两信号互不串扰）；
     - 纯函数层：archive 锚的回位决策（anchorRestoreIndex 复用）与焦点归还
       决策（readerFocusReturnIndex）直接断言；模块级存档 API（stash/peek/
       rearm）与 Timeline 同入口驱动；接线由源码形态断言钉住（t111-6 先例）。

     判别设计（吸收 t111 审查教训——断言必须有变异判别力）：
     - 存档-恢复断言锚定「回到原上下文后 peekReturnAnchor 命中且决策返回
       原索引」，naive 实现（stash 不存档 / 恢复用活锚）必转红；
     - 同上下文重复导航断言 contextChanged 守卫（无守卫 → 恢复信号误 bump）；
     - 切排序断言「信号不 bump + 新键查档为空」（新语境裁定的双重形态）；
     - 关闭信号断言「连续两次关闭必 +1」（nonce 选型判别：id 字段同值不触发）。
     ============================================================ */
  {
    const fs115 = await import('node:fs');
    const src115 = (p) => fs115.readFileSync(new URL(p, import.meta.url), 'utf8');
    const ta115 = await import('../../src/components/timelineAnchor.ts');
    const { RETURN_ANCHOR_ARCHIVE_MAX, anchorRestoreIndex: ari115, readerFocusReturnIndex: rfri115 } = ta115;
    const filterKeyOf115 = (s) => `${s.activeContentLayout}|${s.activeViewFilter}|${s.activeFeedFilter}|${s.timelineFilter}|${s.timelineSort}`;
    const itemsOf115 = () => selectVisibleEntries(store.getState());
    const cnt115 = (s, t) => (s.match(new RegExp(t.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'g')) || []).length;
    const ordered115 = (s, parts) => {
      let i = 0;
      for (const p of parts) { i = s.indexOf(p, i); if (i < 0) return false; i += p.length; }
      return true;
    };
    /* 模块级测试时钟：t111 块已把节流基准推到 NOW+30M，这里从更大刻度起，
       保证 recordTopAnchor 不被上一块残留的节流窗口吞掉 */
    let T115 = NOW + 40000000;

    /* -- (t115-0) X1/X2 接线 + X3 规则文档化（源级防回退，t111-6 同口径） -- */
    const timelineSrc115 = src115('../../src/components/Timeline.tsx');
    const navSrc115 = src115('../../src/store/slices/nav.ts');
    const readerSrc115 = src115('../../src/store/slices/reader.ts');
    const anchorSrc115 = src115('../../src/components/timelineAnchor.ts');
    checkNew('(t115-0) X1 接线（源级）：nav 三处缓存命中恢复按 contextChanged 条件 bump switchRestoreNonce（entries 与信号同一次原子写入；同上下文重复导航不恢复）；Timeline 存档时序 = 先 stash 后 clear（弃锚语义不变，只多归档）',
      cnt115(navSrc115, '...(contextChanged ? { switchRestoreNonce: s.switchRestoreNonce + 1 } : {})') === 3
      && cnt115(navSrc115, 'const contextChanged =') === 3
      && timelineSrc115.indexOf('stashTopAnchorForReturn();') >= 0
      && timelineSrc115.indexOf('stashTopAnchorForReturn();') < timelineSrc115.indexOf('clearTopAnchor();'));
    /* 【TASK-123 改动理由】rearm 调用补第 4 参（卡片内像素偏移随重锚落档——
       恢复落点含 intra-item 偏移，审计 P2-5③）：有序链保护意图不变
       （查档 → 决策 → 程序性滚动抑制 → scrollToIndex → 重锚），字面量随
       调用形态同步更新。 */
    checkNew('(t115-0) X1 消费侧（源级·有序）：switchRestoreNonce effect = 查档(peekReturnAnchor) → 决策(anchorRestoreIndex) → 程序性滚动抑制 → scrollToIndex(align:start) → 重锚(rearmTopAnchor)；image 提前回落',
      ordered115(timelineSrc115, [
        'const switchRestoreNonce = useAppStore((s) => s.switchRestoreNonce);',
        'anchorRestoreIndex(peekReturnAnchor(filterKey), filterKey, items)',
        'suppressNextScrollEvents();',
        "rowVirtualizer.scrollToIndex(idx, { align: 'start' });",
        'rearmTopAnchor(items[idx].id, filterKey, performance.now(), peekReturnAnchor(filterKey)?.offsetPx ?? 0);',
      ]));
    checkNew('(t115-0) X2 接线（源级）：reader.ts 唯一关闭路径 bump readerCloseNonce；Timeline 在 activeArticleId 跟随 effect 记账原选中卡（ref），关闭信号 effect 消费决策（readerFocusReturnIndex → focusCardAt）',
      readerSrc115.includes('readerCloseNonce: s.readerCloseNonce + 1')
      && ordered115(timelineSrc115, [
        'const readerCloseNonce = useAppStore((s) => s.readerCloseNonce);',
        'readerFocusReturnIndex(lastActiveArticleIdRef.current, items)',
        'focusCardAt(idx);',
      ])
      && timelineSrc115.includes('lastActiveArticleIdRef.current = activeArticleId;'));
    checkNew('(t115-0) 画廊回落口径（源级）：三个回位/归还 effect（TASK-111 刷新保位 / TASK-115 切换返回 / 阅读器关闭焦点归还）各有 image 布局提前回落（非虚拟化无定位基建，与 TASK-111 同一口径）',
      cnt115(timelineSrc115, "if (activeContentLayout === 'image') return;") === 3);
    checkNew('(t115-0) X3 规则文档化（代码单点注释）：timelineAnchor.ts 头注 = 全场景规则表（切排序=新语境裁定、锚丢失归零回落、画廊回落、不叠加保证）',
      anchorSrc115.includes('全场景规则表')
      && anchorSrc115.includes('切排序（重拉）')
      && anchorSrc115.includes('新语境')
      && anchorSrc115.includes('存档随 filterKey 键自然失效')
      && anchorSrc115.includes('不叠加保证'));

    /* ---------- (t115-x1a) 存档-恢复：切走再切回，恢复信号 + 存档锚命中原索引 ---------- */
    await resetStore();
    ta115.clearTopAnchor();
    await store.getState().bootstrapFromBackend(); // 8 行；bootstrap 写 'article|all|all' 视图缓存
    const t115K1 = filterKeyOf115(store.getState());
    const t115srn0 = store.getState().switchRestoreNonce;
    T115 += 1000;
    checkNew('(t115-x1a) 前置：顶条锚已记录（101@K1，活锚与 Timeline handleScroll 同一入口）',
      ta115.recordTopAnchor('101', t115K1, T115) === true && ta115.peekTopAnchor()?.id === '101');
    /* 离开上下文（与 Timeline filterKey layout effect 同一入口：先存档后清活锚） */
    ta115.stashTopAnchorForReturn();
    ta115.clearTopAnchor();
    S.listPlan = { mode: 'reject', error: { message: 't115 注入' } };
    store.getState().selectFeed('11'); // 切到 feed-11（缓存 miss，后台 reload 注入失败冻结状态）
    await nTick(10);
    store.getState().selectFeed('all'); // 切回：缓存命中 → entries 同步恢复 + 恢复信号
    const t115stA = store.getState();
    checkNew('(t115-x1a) 存档-恢复（store 层）：缓存命中切回 → switchRestoreNonce +1（entries 与信号同一次原子写入，恢复列表已就位）',
      t115stA.switchRestoreNonce === t115srn0 + 1 && t115stA.entries.length === 8 && t115stA.activeFeedFilter === 'all');
    const t115Kback = filterKeyOf115(t115stA);
    const t115arch = ta115.peekReturnAnchor(t115Kback);
    /* 【TASK-117 改动理由】期望索引随 id 决胜序更新：BASE_ROWS 的 TODAY 并列组
       （101/102 同秒）排序从插入序变为 id 降序（102,101），101 的索引 0 → 1；
       断言意图（存档锚命中恢复列表中的原条目）不变。 */
    checkNew('(t115-x1a) 存档-恢复（决策侧）：按原上下文键查档命中（离开时顶条 101），锚 id 在恢复列表中 → 决策返回其索引（Timeline 据此 scrollToIndex align:start 一次性定位）',
      t115Kback === t115K1 && t115arch?.id === '101' && t115arch?.filterKey === t115K1
      && ari115(t115arch, t115Kback, itemsOf115()) === 1);
    await nTick(10); // 让注入失败的 reload 落定
    S.listPlan = null;

    /* ---------- (t115-x1b) 锚丢失 → 归零回落（不猜） ---------- */
    T115 += 1000;
    ta115.recordTopAnchor('888', t115Kback, T115); // 锚指向已不存在/被筛出的条目
    ta115.stashTopAnchorForReturn();
    ta115.clearTopAnchor();
    S.listPlan = { mode: 'reject', error: { message: 't115 注入' } };
    store.getState().selectFeed('11');
    await nTick(10);
    store.getState().selectFeed('all');
    await nTick(10);
    S.listPlan = null;
    checkNew('(t115-x1b) 锚丢失归零回落（判别）：存档锚 id 不在恢复列表 → anchorRestoreIndex 返回 null（消费侧不动作 = 保持归零 effect 置顶后的顶部；首次进入/无存档同口径回落，绝不猜位置）',
      ta115.peekReturnAnchor(filterKeyOf115(store.getState()))?.id === '888'
      && ari115(ta115.peekReturnAnchor(filterKeyOf115(store.getState())), filterKeyOf115(store.getState()), itemsOf115()) === null);

    /* ---------- (t115-x3c) 同上下文重复导航不恢复（contextChanged 守卫，判别） ---------- */
    const t115srnGuard = store.getState().switchRestoreNonce;
    store.getState().selectFeed('all'); // 已在 all：缓存命中但 contextChanged=false
    checkNew('(t115-x3c) 同上下文重复导航不恢复（判别）：缓存命中但范围段未变 → 恢复信号不 bump（无守卫的 naive 实现会把列表拽回上次离开位置）',
      store.getState().switchRestoreNonce === t115srnGuard && store.getState().activeFeedFilter === 'all');
    await nTick(10);

    /* ---------- (t115-x1c) 切排序（重拉）不恢复——新语境裁定 ---------- */
    const t115srnSort = store.getState().switchRestoreNonce;
    store.getState().toggleTimelineSort(); // filterKey 变（sort 段）+ 清视图缓存 + 重拉
    await nTick(20);
    const t115stSort = store.getState();
    checkNew('(t115-x1c) 切排序不恢复（新语境裁定）：filterKey 已变 = 新语境 → 恢复信号不 bump（重拉不是缓存恢复路径），新键查档为空（进入即归零，裁定无需特判——存档随 filterKey 键自然失效），旧存档仍在旧键下（回原上下文仍可恢复）',
      t115stSort.switchRestoreNonce === t115srnSort && t115stSort.timelineSort === 'oldest'
      && ta115.peekReturnAnchor(filterKeyOf115(t115stSort)) === null
      && ta115.peekReturnAnchor(t115K1)?.id === '888');

    /* ---------- (t115-x2a/b) 阅读器关闭焦点归还 ---------- */
    await resetStore();
    ta115.clearTopAnchor();
    await store.getState().bootstrapFromBackend();
    store.getState().selectArticle('102'); // 打开阅读器（102 在列表 index 1）
    checkNew('(t115-x2a) 前置：阅读器打开且选中 102', store.getState().activeArticleId === '102');
    const t115rcn0 = store.getState().readerCloseNonce;
    store.getState().clearReaderSelection(); // Esc 关闭（App.tsx 唯一关闭路径）
    checkNew('(t115-x2a) 阅读器关闭信号：clearReaderSelection bump readerCloseNonce 且选中清空（滚动不动 = filterKey 未变不触发归零，既有语义）',
      store.getState().readerCloseNonce === t115rcn0 + 1 && store.getState().activeArticleId === null);
    /* 【TASK-117 改动理由】期望索引随 id 决胜序更新：TODAY 并列组 id 降序后 102
       为列表首条（原 index 1 → 0）；断言意图（原卡仍在列表 → 返回其索引）不变。 */
    checkNew('(t115-x2a) 焦点归还原卡（决策）：原卡 102 仍在当前列表 → 决策返回其 index（Timeline 据此 focusCardAt：原卡可见不滚、不可见 scrollToIndex 定位后聚焦）',
      rfri115('102', itemsOf115()) === 0);
    store.getState().selectArticle('103'); // 换一篇打开
    const t115rcn1 = store.getState().readerCloseNonce;
    store.getState().clearReaderSelection(); // 再关（同一篇形态的连续开关）
    checkNew('(t115-x2a) 关闭信号必重触发（选型判别）：连续两次关闭 nonce 连续 +1——若用 activeArticleId 字段本身当信号，同值（开→关→再开同一篇→关）无法重触发 effect，这是选计数器不选 id 字段的理由',
      store.getState().readerCloseNonce === t115rcn1 + 1);
    store.getState().selectFeed('12'); // 阅读器开着时列表已切范围（selectFeed 不清选中；feed-12 与当前 article 布局同布局，列表非空）
    await nTick(20);
    /* 【TASK-117 改动理由】期望列表随 id 决胜序更新：feed-12 的 104/105 同秒并列，
       id 降序后为 [105,104]（原插入序 [104,105]）；断言意图（原卡不在列表 → null）不变。 */
    checkNew('(t115-x2b) 焦点归还回落（判别）：原卡不在当前列表（列表已切换，新范围=[105,104]）→ 决策 null = 不聚焦不滚动（焦点归还无对象，绝不猜）；无归还目标（null id）同口径',
      rfri115('103', itemsOf115()) === null && itemsOf115().map((e) => e.id).join(',') === '105,104'
      && rfri115(null, itemsOf115()) === null);

    /* ---------- (t115-x3a) 与 TASK-111 刷新保位不叠加（双向隔离） ---------- */
    await resetStore();
    ta115.clearTopAnchor();
    await store.getState().bootstrapFromBackend();
    const t115srnIso = store.getState().switchRestoreNonce;
    const t115prnIso = store.getState().positionRestoreNonce;
    ta115.rearmTopAnchor('101', filterKeyOf115(store.getState()), (T115 += 1000));
    ta115.stashTopAnchorForReturn();
    ta115.clearTopAnchor();
    S.listPlan = { mode: 'reject', error: { message: 't115 注入' } };
    store.getState().selectFeed('11');
    await nTick(10);
    store.getState().selectFeed('all'); // 切换返回：只动 switchRestoreNonce
    checkNew('(t115-x3a) 不叠加（出向）：切换返回恢复 bump switchRestoreNonce 但 positionRestoreNonce 纹丝不动（导航路径的 reload 不带 keepReadingPosition，「一次性定位」绝不借用「持续跟踪」通道）',
      store.getState().switchRestoreNonce === t115srnIso + 1
      && store.getState().positionRestoreNonce === t115prnIso);
    await nTick(10);
    S.listPlan = null;
    await store.getState().reloadFromBackend({ keepReadingPosition: true }); // 后台刷新：只动 positionRestoreNonce
    checkNew('(t115-x3a) 不叠加（入向）：keepReadingPosition 刷新落地 bump positionRestoreNonce 但 switchRestoreNonce 纹丝不动（两个 effect 各消费各的信号、各用各的锚——存档 vs 活锚，互不发出互不消费对方信号）',
      store.getState().positionRestoreNonce === t115prnIso + 1
      && store.getState().switchRestoreNonce === t115srnIso + 1);

    /* ---------- (t115-x3b) 存档容量 LRU + rearm 语义（模块级） ---------- */
    T115 += 5000;
    ta115.rearmTopAnchor('r1', 'fk-r', T115);
    checkNew('(t115-x3b) rearm 重锚语义（恢复的支撑）：绕过节流直设活锚并推进节流基准（窗口内到达的 scroll 事件记录被吸收——活锚保持在恢复落点，用户随即再离开时存档的是恢复后位置而非恢复前位置）',
      ta115.peekTopAnchor()?.id === 'r1'
      && ta115.recordTopAnchor('r2', 'fk-r', T115 + 100) === false
      && ta115.peekTopAnchor()?.id === 'r1');
    ta115.clearTopAnchor();
    for (let i = 0; i <= RETURN_ANCHOR_ARCHIVE_MAX; i++) {
      ta115.rearmTopAnchor(`a${i}`, `fk-${i}`, T115 + 100 + i);
      ta115.stashTopAnchorForReturn();
      ta115.clearTopAnchor();
    }
    checkNew(`(t115-x3b) 存档容量 LRU（TASK-111 预算纪律）：上限 ${RETURN_ANCHOR_ARCHIVE_MAX}（与视图缓存键数同值），溢出淘汰最旧上下文（fk-0 已淘汰、fk-MAX 保留）——淘汰只影响该上下文退回「归零回落」，无正确性影响`,
      ta115.peekReturnAnchor('fk-0') === null
      && ta115.peekReturnAnchor(`fk-${RETURN_ANCHOR_ARCHIVE_MAX}`)?.id === `a${RETURN_ANCHOR_ARCHIVE_MAX}`);
    ta115.peekReturnAnchor('fk-1'); // 命中刷新 LRU 新鲜度
    ta115.rearmTopAnchor('aNEW', 'fk-NEW', T115 + 500);
    ta115.stashTopAnchorForReturn();
    ta115.clearTopAnchor();
    checkNew('(t115-x3b) LRU 新鲜度：peek 命中把该上下文刷新为最新使用，随后溢出淘汰的是真正最旧的 fk-2（fk-1 因刚被使用而幸存）',
      ta115.peekReturnAnchor('fk-1')?.id === 'a1' && ta115.peekReturnAnchor('fk-2') === null
      && ta115.peekReturnAnchor('fk-NEW')?.id === 'aNEW');

    await resetStore(); // 夹具复位
  }

  /* ============================================================
     TASK-123（2026-10-07，审计 P2-5）：保位窗口扩展——深页阅读的后台刷新
     不再丢锚。三处机制（均扩展 TASK-111/115，不推翻）：
     ① 保位窗口重取（bootstrap.ts fetchWindowRows）：keepReadingPosition 的
       刷新不再只拉首屏——以发起时游标 loaded 为窗口目标、游标窗口底锚
       lastId 为底边界，keyset 续页重取 [首屏..窗口末]。底边界是判别核心：
       只拉满旧长度会被插头部的新文章把窗口尾部挤出重取范围（锚恰在其间
       照样丢——探针 P7 换形态复发）；底锚命中使窗口按实际插入量自然生长，
       锚 id 必在（除非真被删除）。
     ② 组合修复（nav.ts）：缓存命中恢复后的后台刷新携带 keepReadingPosition
       （重拉走保位窗口路径 → 恢复定位过的锚不因重拉二次丢失）；cache-miss
       与切排序 = 新语境保持无参。切换动作本身依旧不 bump positionRestoreNonce
       ——「同步段隔离 + 刷新落地段按刷新通道回位」两段语义。
     ③ 锚载荷扩展（timelineAnchor.ts + Timeline.tsx）：卡片内像素偏移
       （offsetPx）记录/提交/重锚/存档全链路携带、两个恢复消费点补加；
       250ms 节流加尾沿补记（commitTopAnchor：停滚定稿，抹平节流相位差）。

     证据边界（与 t111/t115 同口径，如实说明）：滚动/虚拟列表本体是 DOM 行为，
     node 回归网不驱动 Timeline 渲染——行为断言两层：store 层走真实动作序列
     （翻页 → 深页窗口 → 刷新/导航重拉，假后端 keyset 语义忠实建模）断言
     窗口存活/锚决策/信号分流；锚模块（record/commit/rearm/stash）与 Timeline
     同入口直接驱动；接线由源码形态断言钉住（t111-6/t115-0 先例）。
     判别设计：t123-1 的「窗口重取」断言在移除 fetchWindowRows 调用（只拉
     首屏）时整体转红（entries 1253→500、锚索引 752→null）；t123-4 的尾沿
     断言在 commit 被节流吸收 / 不推进基准两种变异下分别转红。
     ============================================================ */
  {
    const fs123 = await import('node:fs');
    const src123 = (p) => fs123.readFileSync(new URL(p, import.meta.url), 'utf8');
    const cnt123 = (s, t) => (s.match(new RegExp(t.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'g')) || []).length;
    const ta123 = await import('../../src/components/timelineAnchor.ts');
    const { flipEntryFlag: t123Flip } = await import('../../dist-test/store/internals.js');
    const { viewEntriesCache: t123Cache } = await import('../../dist-test/store/internals.js');
    const filterKeyOf123 = (s) => `${s.activeContentLayout}|${s.activeViewFilter}|${s.activeFeedFilter}|${s.timelineFilter}|${s.timelineSort}`;
    const mkT123Row = (o) => mkRow({ feed_id: 10, ...o });
    /* 夹具：id startId+i，i 越小越新（published_at 逐行递减 60s，无并列） */
    const t123Rows = (count, startId) => Array.from({ length: count }, (_, i) =>
      mkT123Row({ id: startId + i, published_at: iso(NOW - i * 60000), title: `t123-${i}` }));
    /* 模块级测试时钟：t111/t115 块已把节流基准推到 NOW+40M 量级，这里换更大
       刻度起步，保证首个 recordTopAnchor 不被残留节流窗口吞掉 */
    let T123 = NOW + 50000000;

    /* -- (t123-0) 接线（源级防回退，t111-6/t115-0 同口径） -- */
    const timelineSrc123 = src123('../../src/components/Timeline.tsx');
    const navSrc123 = src123('../../src/store/slices/nav.ts');
    const bootstrapSrc123 = src123('../../src/store/slices/bootstrap.ts');
    checkNew('(t123-0) 窗口重取接线（源级）：reloadFromBackend 与 reloadFilteredEntries 的 keepReadingPosition 路径均接入 fetchWindowRows（await 调用 ×2），续页 = keyset 形态（last_published/last_id 成对），底边界条件在循环谓词中（长度未达标 **或** 底锚未覆盖即继续），落地 exhausted 随窗口重取判定（×2）',
      cnt123(bootstrapSrc123, 'await fetchWindowRows(') === 2
      && bootstrapSrc123.includes('rows.some((r) => r.id === bottomId)')
      && bootstrapSrc123.includes('last_published: last.published_at,')
      && bootstrapSrc123.includes('last_id: last.id,')
      && cnt123(bootstrapSrc123, 'articlesExhausted: windowExhausted') === 2);
    checkNew('(t123-0) 导航接线（源级，P2-5②）：nav 三路径「缓存命中恢复后的后台刷新」携带 keepReadingPosition（selectLayout/selectFeed 条件形态 ×2 + selectView 命中分支 ×1，筛选视图透传同构 ×3），cache-miss 与切排序保持无参（新语境首屏起步，规则表裁定）',
      cnt123(navSrc123, 'reloadFromBackend(cached ? { keepReadingPosition: true } : undefined)') === 2
      && cnt123(navSrc123, 'reloadFilteredEntries(view, cached ? { keepReadingPosition: true } : undefined)') === 2
      && cnt123(navSrc123, 'reloadFromBackend({ keepReadingPosition: true })') === 1
      && cnt123(navSrc123, 'reloadFilteredEntries(view, { keepReadingPosition: true })') === 1
      && cnt123(navSrc123, 'get().reloadFromBackend().catch(') === 2
      && cnt123(navSrc123, 'get().reloadFilteredEntries(view).catch(') === 2);
    checkNew('(t123-0) Timeline 尾沿补记接线（源级）：滚动静默 ANCHOR_RECORD_THROTTLE_MS 后 commitTopAnchor 定稿（定时器重置 = 尾沿语义、回调重读实时 store getState、上下文切换不复活锚、handleScroll 调度）',
      timelineSrc123.includes('commitTopAnchor(top.id, liveFilterKey, performance.now(), measureAnchorOffsetPx(topIndex));')
      && timelineSrc123.includes('}, ANCHOR_RECORD_THROTTLE_MS);')
      && timelineSrc123.includes('scheduleAnchorCommit(filterKey);')
      && timelineSrc123.includes('if (liveFilterKey !== scheduledFilterKey) return;'));

    /* ---------- (t123-1) 探针 P7 本体：深页读位刷新保位（判别） ---------- */
    await resetStore();
    ta123.clearTopAnchor();
    S.backendRows = t123Rows(1250, 1000);
    await store.getState().bootstrapFromBackend(); // 首批 500（1000..1499）
    await store.getState().loadMoreArticles();     // 续拉 → 1000..1999（深页：窗口跨页）
    T123 += 1000;
    const t123fk1 = filterKeyOf123(store.getState());
    checkNew('(t123-1) 前置（探针场景建模）：翻页加载 1000 篇（exhausted=false，集合还有余量）、读位在第 750 篇（index 749 = id 1749，卡片内偏移 137px）',
      store.getState().entries.length === 1000 && store.getState().entries[749].id === '1749'
      && store.getState().articlesCursor['article|all']?.loaded === 1000
      && store.getState().articlesExhausted === false
      && ta123.recordTopAnchor('1749', t123fk1, T123, 137) === true
      && ta123.peekTopAnchor()?.offsetPx === 137);
    /* 后台刷新：3 篇新文章插头部（feeds-updated 同形态） */
    S.backendRows = [
      mkT123Row({ id: 9000, published_at: iso(NOW + 600000) }),
      mkT123Row({ id: 9001, published_at: iso(NOW + 599000) }),
      mkT123Row({ id: 9002, published_at: iso(NOW + 598000) }),
      ...S.backendRows,
    ];
    const t123segStart = S.invokeCalls.filter((c) => c.cmd === 'list_articles').length;
    const t123nonce1 = store.getState().positionRestoreNonce;
    await store.getState().reloadFromBackend({ keepReadingPosition: true });
    const t123seg = S.invokeCalls.filter((c) => c.cmd === 'list_articles').slice(t123segStart);
    checkNew('(t123-1) 刷新 wire（判别）：keepReadingPosition 重取发出「首屏 + 2 个 keyset 续页」共 3 次 list_articles——首屏 offset=0/limit=500；续页 last_published/last_id 成对（1496/1996）、无 OFFSET（与 loadMoreArticles 同形态）——移除窗口重取（只拉首屏）时此断言与下方窗口断言同红',
      t123seg.length === 3
      && t123seg[0].args.args.offset === 0 && t123seg[0].args.args.limit === 500 && t123seg[0].args.args.last_id === undefined
      && t123seg[1].args.args.last_id === 1496 && t123seg[1].args.args.last_published === iso(NOW - 496 * 60000)
      && t123seg[1].args.args.offset === undefined && t123seg[1].args.args.limit === 500
      && t123seg[2].args.args.last_id === 1996);
    const t123st1 = store.getState();
    checkNew('(t123-1) P7 本体（判别）：窗口重取后 entries=1253（**不缩回 500**——探针的「刷新主动丢阅读窗口」被消除；插头部使窗口按实际插入量生长）、新文章居头部、深页锚 id 1749 仍在列表',
      t123st1.entries.length === 1253
      && t123st1.entries[0].id === '9000' && t123st1.entries[1].id === '9001' && t123st1.entries[2].id === '9002'
      && t123st1.entries.some((e) => e.id === '1749'));
    checkNew('(t123-1) 保位恢复（判别）：positionRestoreNonce +1、锚载荷（id 1749 / offsetPx 137）原样幸存、决策函数返回新索引 752（头部插入 3 行的漂移被校正——Timeline 据此 scrollToIndex + offsetPx 补偏原位还原）；naive「只拉满旧长度」会把窗口尾部 3 行挤出（1749 恰在其后不变——但底锚 1999 之后的行随 keyset 短页一并收敛，锚索引仍可判别：缩回 500 形态下本断言全红）',
      t123st1.positionRestoreNonce === t123nonce1 + 1
      && ta123.peekTopAnchor()?.id === '1749' && ta123.peekTopAnchor()?.offsetPx === 137
      && ta123.anchorRestoreIndex(ta123.peekTopAnchor(), filterKeyOf123(t123st1), selectVisibleEntries(t123st1)) === 752);
    checkNew('(t123-1) 游标一致（判别）：cursor.loaded=1253 / 底锚推进到窗口末行 2249 / exhausted=true（keyset 短页真实判定：末页 253<500 = 集合尽）——重取后游标与 entries 原子一致，续拉自窗口末无缝衔接',
      t123st1.articlesCursor['article|all']?.loaded === 1253
      && t123st1.articlesCursor['article|all']?.lastId === 2249
      && t123st1.articlesExhausted === true);

    /* ---------- (t123-2) 切换返回 + 导航后台重拉组合：锚不丢（审计 P2-5②） ---------- */
    await resetStore();
    ta123.clearTopAnchor();
    S.backendRows = t123Rows(1250, 1000);
    await store.getState().bootstrapFromBackend();
    await store.getState().loadMoreArticles(); // 深页窗口 1000（= 预算边界，缓存不截断）
    t123Flip('1000', 'isStarred');             // 真实翻旗 → syncCurrentViewCache 把整个深页窗口落键（t111 同口径驱动）
    T123 += 1000;
    const t123fk2 = filterKeyOf123(store.getState());
    checkNew('(t123-2) 前置：深页窗口 1000 篇已随翻旗同步进视图缓存（1000=预算边界不截断）、顶条锚记录 index 749（id 1749，卡片内偏移 60px）',
      store.getState().entries.length === 1000
      && t123Cache.get('article|all|all')?.entries.length === 1000
      && t123Cache.get('article|all|all')?.cursor.loaded === 1000
      && ta123.recordTopAnchor('1749', t123fk2, T123, 60) === true);
    ta123.stashTopAnchorForReturn(); // 离开上下文（与 Timeline layout effect 同序：先存档后清）
    ta123.clearTopAnchor();
    S.listPlan = { mode: 'defer' };
    store.getState().selectFeed('11'); // 切走：feed-11 缓存 miss → 后台 reload 挂起（defer）
    await nTick(0);
    const t123srn2 = store.getState().switchRestoreNonce;
    const t123prn2pre = store.getState().positionRestoreNonce;
    store.getState().selectFeed('all'); // 切回：缓存命中 → 深页窗口同步恢复 + 恢复信号；后台刷新携带保位（挂起）
    await nTick(0);
    const t123st2a = store.getState();
    checkNew('(t123-2) 切回恢复：switchRestoreNonce +1（恢复信号）、深页窗口同步恢复（entries=1000 不缩回）、存档锚命中原索引 749 且 intra-item 偏移 60 经存档往返保留；两次切换动作对 positionRestoreNonce 零触碰（同步段隔离——信号只由刷新落地发出）',
      t123st2a.switchRestoreNonce === t123srn2 + 1
      && t123st2a.positionRestoreNonce === t123prn2pre
      && t123st2a.entries.length === 1000 && t123st2a.entries[749].id === '1749'
      && ta123.peekReturnAnchor(t123fk2)?.id === '1749' && ta123.peekReturnAnchor(t123fk2)?.offsetPx === 60
      && ta123.anchorRestoreIndex(ta123.peekReturnAnchor(t123fk2), t123fk2, selectVisibleEntries(t123st2a)) === 749);
    /* Timeline 切换返回消费存档后的重锚（与 switchRestore effect 同形态：含偏移） */
    ta123.rearmTopAnchor('1749', t123fk2, (T123 += 1000), 60);
    /* 导航后台重拉落地：期间同步插入 1 篇新文章（切回窗口内 feeds-updated 的真实形态） */
    S.backendRows = [mkT123Row({ id: 9000, published_at: iso(NOW + 600000) }), ...S.backendRows];
    const t123prn2 = store.getState().positionRestoreNonce;
    checkNew('(t123-2) 组合场景成立：切走的 feed-11 reload 与切回的保位刷新都在途（defer 挂起 ×2）——P2-5② 缺陷窗口：恢复定位已做、重拉即将落地',
      S.pendingList.length === 2 && S.pendingList[1].args.offset === 0 && S.pendingList[1].args.limit === 500);
    const t123stale11 = S.pendingList[0];    // feed-11 reload（代际已过期）
    const t123navFirst = S.pendingList[1];   // 'all' 保位窗口刷新（导航后台刷新）
    t123stale11.resolve(queryRows(t123stale11.args)); // 落地时被查询代际整体丢弃
    await nTick(10);
    t123navFirst.resolve(queryRows(t123navFirst.args)); // 首屏放行 → keyset 续页挂起 → 逐页放行至窗口收敛
    await nTick(10);
    while (S.pendingList.length) {
      const p = S.pendingList.shift();
      p.resolve(queryRows(p.args));
      await nTick(10);
    }
    await nTick(10);
    const t123st2b = store.getState();
    checkNew('(t123-2) 重拉落地（P2-5② 本体判别）：导航后台刷新按保位窗口重取——entries=1251（**不缩回 500**：修前「重拉再次删除锚」的第二现场）、新文章插头部、锚 id 1749 幸存',
      t123st2b.entries.length === 1251
      && t123st2b.entries[0].id === '9000'
      && t123st2b.entries.some((e) => e.id === '1749'));
    checkNew('(t123-2) 组合回位（两段语义）：刷新落地 bump positionRestoreNonce（内容刷新通道——切换动作本身依旧沉默，同步段隔离语义不变）、活锚（重锚含偏移 60）在新窗口索引 750（Timeline 据此二次回位校正漂移——只订阅 switchRestoreNonce 的 effect 无需重拉后重试）',
      t123st2b.positionRestoreNonce === t123prn2 + 1
      && ta123.peekTopAnchor()?.id === '1749' && ta123.peekTopAnchor()?.offsetPx === 60
      && ta123.anchorRestoreIndex(ta123.peekTopAnchor(), filterKeyOf123(t123st2b), selectVisibleEntries(t123st2b)) === 750);
    S.listPlan = null;
    await resetStore(); // defer 残留不复带给后续块

    /* ---------- (t123-3) intra-item 像素偏移：记录/存档/重锚全链路（模块级） ---------- */
    ta123.clearTopAnchor();
    T123 += 1000;
    checkNew('(t123-3) 偏移入锚（模块级）：recordTopAnchor 四参载荷——锚携带卡片内像素偏移（审计③：锚只存 id，长社交正文只能恢复卡片顶）',
      ta123.recordTopAnchor('mx', 'fk-mx', T123, 42) === true
      && ta123.peekTopAnchor()?.offsetPx === 42
      && ta123.peekTopAnchor()?.id === 'mx');
    ta123.stashTopAnchorForReturn();
    ta123.clearTopAnchor();
    checkNew('(t123-3) 偏移经存档往返：stash → peekReturnAnchor 保留 offsetPx（切换返回恢复的偏移数据源）',
      ta123.peekReturnAnchor('fk-mx')?.offsetPx === 42 && ta123.peekReturnAnchor('fk-mx')?.id === 'mx');
    T123 += 1000;
    ta123.rearmTopAnchor('my', 'fk-my', T123, 77);
    checkNew('(t123-3) 偏移随重锚：rearmTopAnchor 四参——恢复消费点以恢复落点偏移重锚（活锚 = 停滚真实位置）；重锚推进节流基准的既有语义不变（窗口内 record 被吸收，偏移不被冲掉）',
      ta123.peekTopAnchor()?.id === 'my' && ta123.peekTopAnchor()?.offsetPx === 77
      && ta123.recordTopAnchor('mz', 'fk-my', T123 + 100, 5) === false
      && ta123.peekTopAnchor()?.offsetPx === 77);
    checkNew('(t123-3) Timeline 恢复消费点接线（源级）：滚动记录带卡片内偏移（measureAnchorOffsetPx 单点口径）、刷新回位补偏（applyAnchorOffsetPx(peekTopAnchor)）、切换返回重锚带偏移 + 补偏（peekReturnAnchor）',
      timelineSrc123.includes('recordTopAnchor(topItem.id, filterKey, performance.now(), measureAnchorOffsetPx(topIndex))')
      && timelineSrc123.includes('applyAnchorOffsetPx(peekTopAnchor()?.offsetPx ?? 0);')
      && timelineSrc123.includes('rearmTopAnchor(items[idx].id, filterKey, performance.now(), peekReturnAnchor(filterKey)?.offsetPx ?? 0);')
      && timelineSrc123.includes('applyAnchorOffsetPx(peekReturnAnchor(filterKey)?.offsetPx ?? 0);'));

    /* ---------- (t123-4) 节流尾沿补记 commitTopAnchor（判别） ---------- */
    ta123.clearTopAnchor();
    T123 += 1000;
    const t123fk4 = 'fk-t123-4';
    checkNew('(t123-4) 缺陷成因建模：节流 lead 锚滞留——窗口内最后一次 record 被吸收，活锚停在较早位置（审计：停滚后立即切换还可能记到较早的位置）',
      ta123.recordTopAnchor('c1', t123fk4, T123) === true
      && ta123.recordTopAnchor('c2', t123fk4, T123 + 100, 30) === false
      && ta123.peekTopAnchor()?.id === 'c1');
    ta123.commitTopAnchor('c2', t123fk4, T123 + 240, 30);
    checkNew('(t123-4) 尾沿补记（判别本体）：commitTopAnchor 无条件写入（绕过节流）——最终停留位置（含偏移 30）落锚，与节流相位无关；变异「commit 被节流吸收」即红',
      ta123.peekTopAnchor()?.id === 'c2' && ta123.peekTopAnchor()?.offsetPx === 30);
    checkNew('(t123-4) commit 推进节流基准：commit 后窗口内 record 照常被吸收（与 rearm 同语义——停滚定稿不被随后同位置采样覆盖回较早值）、窗口过期恢复记录；变异「commit 不推进基准」即红',
      ta123.recordTopAnchor('c3', t123fk4, T123 + 340) === false
      && ta123.recordTopAnchor('c3', t123fk4, T123 + 490) === true
      && ta123.peekTopAnchor()?.id === 'c3');

    /* ---------- (t123-5) 缓存截断窗口边界：budget 截断 → 恢复 → 续拉衔接 → 保位刷新 ---------- */
    await resetStore();
    ta123.clearTopAnchor();
    S.backendRows = t123Rows(1501, 3000); // 1501 行：3 页后余 1 行
    await store.getState().bootstrapFromBackend();
    await store.getState().loadMoreArticles();
    await store.getState().loadMoreArticles(); // loaded=1500（3000..4499），exhausted=false
    t123Flip('3000', 'isStarred');             // 落缓存：1500 > 预算 1000 → 尾部截断，元数据记录真实游标（t111-1 语义）
    S.listPlan = { mode: 'reject', error: { message: 't123 注入' } };
    store.getState().selectFeed('11');         // 切走（reload 注入失败冻结状态）
    await nTick(10);
    store.getState().selectFeed('all');        // 切回：缓存命中 → 截断快照恢复（1000 条）+ 游标按记录值 1500
    await nTick(10);
    S.listPlan = null;
    const t123st5a = store.getState();
    checkNew('(t123-5) 前置（截断恢复形态）：恢复 entries=1000（budget 截断）但游标=记录值 loaded=1500/底锚 4499（不能只保留截断长度）',
      t123st5a.entries.length === 1000 && t123st5a.entries[0].id === '3000'
      && t123st5a.articlesCursor['article|all']?.loaded === 1500
      && t123st5a.articlesCursor['article|all']?.lastId === 4499
      && t123st5a.articlesExhausted === false);
    await store.getState().loadMoreArticles(); // 恢复后续拉衔接：keyset 锚 4499 → 追加 4500（t111-2 同衔接，本卡窗口下复核）
    const t123st5b = store.getState();
    checkNew('(t123-5) 恢复续拉衔接：keyset 自记录底锚 4499 续 1 行（4500）、loaded=1501、exhausted=true——恢复后的窗口边界仍以记录游标为准',
      t123st5b.entries.length === 1001 && t123st5b.entries.at(-1).id === '4500'
      && t123st5b.articlesCursor['article|all']?.loaded === 1501
      && t123st5b.articlesCursor['article|all']?.lastId === 4500
      && t123st5b.articlesExhausted === true);
    T123 += 1000;
    checkNew('(t123-5) 前置：锚记录在 index 999（id 3999，偏移 25）——恢复+续拉后的真实读位',
      ta123.recordTopAnchor('3999', filterKeyOf123(t123st5b), T123, 25) === true);
    S.backendRows = [mkT123Row({ id: 9000, published_at: iso(NOW + 600000) }), ...S.backendRows];
    const t123nonce5 = store.getState().positionRestoreNonce;
    await store.getState().reloadFromBackend({ keepReadingPosition: true }); // 窗口目标=记录游标 1501、底边界=4500
    const t123st5c = store.getState();
    checkNew('(t123-5) 保位刷新按记录窗口重取（判别）：窗口目标取**记录游标 loaded（1501）**而非恢复快照长度（1001）+ 底边界收敛——entries=1502（被截断的中段 4000..4499 随窗口重取回归，4100 在列）、游标 loaded=1502/底锚 4500、exhausted=true（keyset 短页 2<500 真实判定——按「行数<页大小」近似会把 1502 行误判成未到底）',
      t123st5c.entries.length === 1502
      && t123st5c.entries.some((e) => e.id === '4100') && t123st5c.entries.at(-1).id === '4500'
      && t123st5c.articlesCursor['article|all']?.loaded === 1502
      && t123st5c.articlesCursor['article|all']?.lastId === 4500
      && t123st5c.articlesExhausted === true);
    checkNew('(t123-5) 截断边界下保位：nonce +1、锚（3999/偏移 25）在新窗口索引 1000（插头部 1 行的漂移被校正）——budget 截断、恢复、续拉、刷新四段衔接后阅读位置不丢',
      t123st5c.positionRestoreNonce === t123nonce5 + 1
      && ta123.peekTopAnchor()?.offsetPx === 25
      && ta123.anchorRestoreIndex(ta123.peekTopAnchor(), filterKeyOf123(t123st5c), selectVisibleEntries(t123st5c)) === 1000);
    ta123.clearTopAnchor();

    /* ---------- (t123-6) 窗口重取失败可见性（R1 修复轮：TASK-067 N10 缺陷类不重开） ----------
       R0 审查 finding：fetchWindowRows 的 await 在首屏 toast catch 之外——续页
       IPC 失败被调用方 .catch(()=>{}) 静默吞掉。注入手法：rejectWhen 按 (cmd,
       args) 谓词放行首屏（offset=0）、只拒续页（带 last_id）——与 p3-f2 同一
       注入形态；判别 = toast 可见性（rethrow 本身修前也发生，toast 是修复本体）。 */
    await resetStore();
    ta123.clearTopAnchor();
    S.backendRows = t123Rows(1250, 1000);
    await store.getState().bootstrapFromBackend();
    await store.getState().loadMoreArticles(); // 深页窗口 1000（1000..1999）
    S.backendRows = [mkT123Row({ id: 9000, published_at: iso(NOW + 600000) }), ...S.backendRows];
    S.rejectWhen = (cmd, a) => cmd === 'list_articles' && a?.args?.last_id != null; // 首屏放行、续页 reject
    const t123nonce6 = store.getState().positionRestoreNonce;
    const t123toast6 = store.getState().toasts.length;
    let t123r6rethrown = false;
    await store.getState().reloadFromBackend({ keepReadingPosition: true }).catch(() => { t123r6rethrown = true; });
    const t123st6 = store.getState();
    checkNew('(t123-6) 续页失败可见（判别本体）：首屏成功、续页 IPC reject → toast 可见（与首屏失败同一「刷新失败：」文案前缀）+ promise 重抛（调用方 .catch 命中）——修前 toast 缺失即红',
      t123r6rethrown === true
      && t123st6.toasts.length === t123toast6 + 1
      && t123st6.toasts.at(-1)?.text.startsWith('刷新失败：'));
    checkNew('(t123-6) 失败不缩窗不假到底：entries 保持 1000（旧快照原样、锚 id 1749 仍在）、游标 loaded=1000 不动、exhausted 不被写成 true、保位信号不 bump（未落地不回位）——失败刷新与成功刷新同一保位语义',
      t123st6.entries.length === 1000 && t123st6.entries[0].id === '1000'
      && t123st6.entries.some((e) => e.id === '1749')
      && t123st6.articlesCursor['article|all']?.loaded === 1000
      && t123st6.articlesExhausted === false
      && t123st6.positionRestoreNonce === t123nonce6);
    S.rejectWhen = null;

    await resetStore(); // 夹具复位
  }
}
