// tools/frontend-tests/settings-and-feeds.mjs
// 领域模块：设置合并/校验与订阅源、分类增删
// OPT-016C 拆分自 tools/frontend-regression.mjs（旧行区间 1589-1706、2298-2310、2983-3036、3038-3063）；
// 断言名称/条件文本原样迁移，仅做路径深度适配（import.meta.url 与动态 import 深一层）与
// 共享可变状态的 S. 归属重写。数据所有权：共享假后端/夹具/记录归 harness（见 harness.mjs 头注），
// 本模块不 import 第二份 store；域内自带夹具（大行集等）仍在本模块内独立构造与复位。
export const id = 'settings-and-feeds';

export async function run(ctx) {
  const { S, store, checkNew, nTick, resetStore, bootFixture, mkRow, overrideGlobal } = ctx;
  await ctx.useMainBackend();

  /* ============================================================
     (k) 设置合并与校验（bootstrapSettings / updateSettings）
     ============================================================ */
  await resetStore();
  S.settingsRaw = JSON.stringify({
    themeMode: 'light', fontSize: '18', lineHeight: 200, bogusKey: 5,
    maxWidth: 760, startupView: 'starred', hideReadOnStartup: false,
  });
  await store.getState().bootstrapSettings();
  const k1 = store.getState();
  checkNew('(k) 逐键合并：类型相符生效、类型不符回落默认（fontSize 收到字符串 → 保留 16）',
    k1.settings.themeMode === 'light' && k1.settings.lineHeight === 200
    && k1.settings.fontSize === 16 && !('bogusKey' in k1.settings));
  checkNew('(k) maxWidth 旧默认迁移：760 → 860', k1.settings.maxWidth === 860);
  checkNew('(k) startupView 白名单生效：合法值直接应用为启动视图', k1.activeViewFilter === 'starred');
  checkNew('(k) hideReadOnStartup=false → 启动时间流显示全部（不隐藏已读）', k1.timelineFilter === 'all');

  S.settingsRaw = JSON.stringify({ startupView: 'bogus' });
  await store.getState().bootstrapSettings();
  checkNew('(k) 非法 startupView 被白名单拦下：activeViewFilter 保持原值不被污染',
    store.getState().activeViewFilter === 'starred');
  /* 【改动理由】原断言把 D5 的现状（读回路径只做 typeof 拦截、非法值照样写进
     settings.startupView）写成了期望，标注为「观察项」。D5 修复后读回路径与
     写入路径共用同一张校验表（settingsValidation），非法值不再落进 settings，
     故改为断言「不被污染」（与上一条 activeViewFilter 的判定同向）。 */
  checkNew('(k) 非法 startupView 也不写进 settings（读回与写入共用同一张校验表 —— D5 修复项）',
    store.getState().settings.startupView === 'starred');

  S.settingsRaw = JSON.stringify({ maxWidth: 900, hideReadOnStartup: true });
  await store.getState().bootstrapSettings();
  checkNew('(k) 显式 maxWidth=900 不被迁移覆盖；hideReadOnStartup=true 回到未读筛选',
    store.getState().settings.maxWidth === 900 && store.getState().timelineFilter === 'unread');

  store.getState().updateSettings({ themeMode: 'dark' });
  S.settingsRaw = null;
  await store.getState().bootstrapSettings();
  checkNew('(k) 后端无存量设置时不改动内存设置（不把默认值反向写回）',
    store.getState().settings.themeMode === 'dark' && store.getState().settings.maxWidth === 900);

  // R1：console.error 覆盖必须 finally 复原（bootstrapSettings 的 reject 路径也保全）
  let kErrCount = 0;
  const restoreConsoleError = overrideGlobal(console, 'error', () => { kErrCount += 1; });
  try {
    S.settingsRaw = '{ 坏 JSON';
    await store.getState().bootstrapSettings();
  } finally {
    restoreConsoleError();
  }
  checkNew('(k) 坏 JSON 被 try/catch 吞掉：记录错误但不破坏当前设置',
    kErrCount === 1 && store.getState().settings.themeMode === 'dark'
    && store.getState().settings.maxWidth === 900);

  S.invokeCalls.length = 0;
  store.getState().updateSettings({ fontSize: 20, themeMode: 'light' });
  await nTick(0);   // api.setSetting 内部 await getInvoke()
  const kWrite = S.invokeCalls.find((c) => c.cmd === 'set_setting');
  const kPayload = kWrite ? JSON.parse(kWrite.args.value) : {};
  checkNew('(k) updateSettings 合并进内存并以单键 JSON 全量落库（含新值且不丢其他键）',
    store.getState().settings.fontSize === 20 && kWrite?.args.key === 'app_settings'
    && kPayload.fontSize === 20 && kPayload.themeMode === 'light'
    && typeof kPayload.markReadOnOpen === 'boolean' && kPayload.listWidth === store.getState().settings.listWidth);

  /* ---------- D5：updateSettings 运行时校验（写入与读回对称） ---------- */
  const k5 = await import('../../dist-test/store/settingsValidation.js');
  const kTypes = await import('../../dist-test/types.js');
  const k5Before = store.getState().settings;
  S.invokeCalls.length = 0;
  store.getState().updateSettings({ fontSize: -5, refreshInterval: 0, fetchConcurrency: 99, listWidth: 99999 });
  const k5Bad = store.getState().settings;
  checkNew('(D5) 越界数值被写入路径拦下：fontSize/refreshInterval/fetchConcurrency/listWidth 保持原值',
    k5Bad.fontSize === k5Before.fontSize && k5Bad.refreshInterval === k5Before.refreshInterval
    && k5Bad.fetchConcurrency === k5Before.fetchConcurrency && k5Bad.listWidth === k5Before.listWidth);
  await nTick(0);   // api.setSetting 内部 await getInvoke()：等一拍才能观察到「有没有落库」
  checkNew('(D5) 整份补丁全非法 → 连落库 IPC 都不发（不留无效写、不空转落库）',
    S.invokeCalls.filter((c) => c.cmd === 'set_setting').length === 0);
  store.getState().updateSettings({ themeMode: 'neon', syncMode: 'p2p', defaultOpenMode: 'pdf', startupView: 'article' });
  const k5Enum = store.getState().settings;
  checkNew('(D5) 非法枚举被拦下：themeMode/syncMode/defaultOpenMode/startupView 保持原值',
    k5Enum.themeMode === k5Before.themeMode && k5Enum.syncMode === k5Before.syncMode
    && k5Enum.defaultOpenMode === k5Before.defaultOpenMode && k5Enum.startupView === k5Before.startupView);
  await nTick(0);
  store.getState().updateSettings({ bogusKey: 1, fontSize: 20 });
  await nTick(0);   // 让这次落库 IPC 先落地，避免与下一段的调用计数串台
  checkNew('(D5) 未知键不进 settings（不认识的键不落库），同一补丁里的合法键照常生效',
    !('bogusKey' in store.getState().settings) && store.getState().settings.fontSize === 20);
  S.invokeCalls.length = 0;
  store.getState().updateSettings({ fontSize: 13, lineHeight: 240, maxWidth: 1100, refreshInterval: 5, fetchConcurrency: 1, listWidth: 280 });
  await nTick(0);   // api.setSetting 内部 await getInvoke()
  const k5Edge = store.getState().settings;
  checkNew('(D5) 滑杆边界值合法可写：13px / 240% / 1100px / 5min / 1路 / 280px',
    k5Edge.fontSize === 13 && k5Edge.lineHeight === 240 && k5Edge.maxWidth === 1100
    && k5Edge.refreshInterval === 5 && k5Edge.fetchConcurrency === 1 && k5Edge.listWidth === 280);
  checkNew('(D5) 合法补丁照常落库（校验不误伤正常路径）',
    S.invokeCalls.filter((c) => c.cmd === 'set_setting').length === 1);
  const kSetKeys = Object.keys(store.getState().settings);
  checkNew('(D5) 校验表与设置键一一对应（漏配校验器会被 tsc 的 Record<keyof SettingsState,…> 拦下）',
    kSetKeys.every((key) => typeof k5.SETTINGS_VALIDATORS[key] === 'function')
    && kSetKeys.length === Object.keys(k5.SETTINGS_VALIDATORS).length);

  /* ---------- D4：启动视图白名单与设置页下拉同源 ---------- */
  checkNew('(D4) 死选项 article 已从两侧移除；余下取值都能通过校验（含此前无 UI 入口的 starred）',
    !kTypes.STARTUP_VIEW_OPTIONS.some((o) => o.value === 'article')
    && kTypes.STARTUP_VIEW_OPTIONS.length === 4
    && kTypes.STARTUP_VIEW_OPTIONS.every((o) => k5.isValidSetting('startupView', o.value))
    && kTypes.STARTUP_VIEW_OPTIONS.some((o) => o.value === 'starred')
    && !k5.isValidSetting('startupView', 'article'));
  /* 先显式落一个合法值作为对照基准（修前 updateSettings 会把它改成 'article'） */
  store.getState().updateSettings({ startupView: 'today' });
  const k4Keep = store.getState().settings.startupView;
  store.getState().updateSettings({ startupView: 'article' });
  checkNew('(D4) 写入路径拒绝死选项（settings.startupView 不落 article）',
    k4Keep !== 'article' && store.getState().settings.startupView === k4Keep);
  S.settingsRaw = JSON.stringify({ startupView: 'article', fontSize: 18 });
  await store.getState().bootstrapSettings();
  checkNew('(D4) 读回路径同样拒绝 article（旧库里的历史值不再进入 settings）',
    store.getState().settings.startupView === k4Keep);
  checkNew('(D4) 同一份读回里的合法键照常生效（fontSize 18）', store.getState().settings.fontSize === 18);
  S.settingsRaw = JSON.stringify({ startupView: 'starred' });
  await store.getState().bootstrapSettings();
  checkNew('(D4) 收藏视图可作启动视图：白名单里的取值确实被应用为 activeViewFilter',
    store.getState().activeViewFilter === 'starred');

  /* ---------- P2-7：版本号未就绪/不可比时不再误判「有更新」 ---------- */
  const { compareVersions, isComparableVersion, shouldOfferUpdate } = await import('../../dist-test/components/settings/compareVersions.js');
  checkNew('(P2-7) 修前误判根因可复现：compareVersions(remote, "") 把空串当 0.0.0 → 恒判远端更新',
    compareVersions('0.9.0', '') > 0 && !isComparableVersion(''));
  checkNew('(P2-7) 本地版本未就绪（空串）→ 不给「有更新」结论（未知 ≠ 有更新）',
    shouldOfferUpdate('0.9.0', '') === false && shouldOfferUpdate('0.9.0', '   ') === false);
  checkNew('(P2-7) 不可比版本（非数字点分 / 回退值以外的脏数据）同样不误判',
    shouldOfferUpdate('0.9.0', 'v0.8.0') === false && shouldOfferUpdate('bad-tag', '0.8.0') === false);
  checkNew('(P2-7) 两端都可比时判定照旧：远端更高 → true，同版/更低 → false',
    shouldOfferUpdate('0.9.0', '0.8.0') === true && shouldOfferUpdate('0.8.0', '0.8.0') === false
    && shouldOfferUpdate('0.7.9', '0.8.0') === false);
  checkNew('(P2-7) 多段版本号（0.10.1 > 0.9.9）比较正确，不走字符串序',
    shouldOfferUpdate('0.10.1', '0.9.9') === true && shouldOfferUpdate('0.9.9', '0.10.1') === false);

    /* fix-1（自检 P1-1）：空 catId（全新安装 0 分类时 AddFeedModal 的实参）必须发
       folder_id=null —— 后端 add_feed 对 None 有「自动落到未分类（不存在则建）」
       兜底；修前 Number('')===0 直传，触发 feeds.folder_id 外键违约，首用添加必败。
       断言能抓住回退：payload.folderId 一旦回到 0（回归 Number('') 直传），即红。 */
    await resetStore();
    await store.getState().addFeed('', 'https://example.com/rss.xml', '', 'inherit', false, false, false);
    const fix1Call = S.invokeCalls.find((c) => c.cmd === 'add_feed');
    checkNew('(fix-1) 空 catId 的 addFeed payload.folderId === null（后端 None→未分类兜底可达；修前为 0）',
      !!fix1Call && fix1Call.args.folderId === null && fix1Call.args.feedUrl === 'https://example.com/rss.xml');
    /* 有数字 id 的路径不受影响 */
    S.invokeCalls.length = 0;
    await store.getState().addFeed('cat-2', 'https://example.com/b.xml', '', 'inherit', false, false, false);
    const fix1b = S.invokeCalls.find((c) => c.cmd === 'add_feed');
    checkNew('(fix-1) 数字 catId 照旧映射（cat-2 → folderId=2）',
      !!fix1b && fix1b.args.folderId === 2);

    /* fix-3（自检 P2-1）：删除当前正浏览的分类/订阅源 → 范围与选中复位（与 mock
       分支同口径）。修前 tauri 分支不清理，entries 按已消失的范围过滤恒为空，
       时间流停在幽灵范围。断言能抓住回退：删掉复位 set 后 activeFeedFilter 仍为
       已删 id，两条断言即红。 */
    await bootFixture();
    store.getState().selectFeed('cat-1');
    await nTick(20);
    store.getState().selectArticle('101');
    checkNew('(fix-3) 前置：已进入 cat-1 范围并选中文章',
      store.getState().activeFeedFilter === 'cat-1' && store.getState().activeArticleId === '101');
    await store.getState().deleteCategory('cat-1');
    await nTick(20);
    checkNew('(fix-3) 删除正浏览的分类 → activeFeedFilter 复位 all 且清空选中（修前停在幽灵范围）',
      store.getState().activeFeedFilter === 'all' && store.getState().activeArticleId === null);
    /* 负向：删除非活动分类不影响当前范围 */
    await bootFixture();
    store.getState().selectFeed('cat-2');
    await nTick(20);
    await store.getState().deleteCategory('cat-1');
    await nTick(20);
    checkNew('(fix-3) 删除非活动分类 → 当前范围保持不变',
      store.getState().activeFeedFilter === 'cat-2');
    /* deleteFeed 同口径 */
    await bootFixture();
    store.getState().selectFeed('10');
    await nTick(20);
    store.getState().selectArticle('101');
    await store.getState().deleteFeed('cat-1', '10');
    await nTick(20);
    checkNew('(fix-3) 删除正浏览的订阅源 → 范围复位 all + 清空选中',
      store.getState().activeFeedFilter === 'all' && store.getState().activeArticleId === null);
    await bootFixture();
    store.getState().selectFeed('11');
    await nTick(20);
    await store.getState().deleteFeed('cat-1', '10');
    await nTick(20);
    checkNew('(fix-3) 删除非活动订阅源 → 当前范围保持不变',
      store.getState().activeFeedFilter === '11');

    /* fix-4（自检 P2-2）：reloadFilteredEntries 的口径守卫——「范围×布局」在途时
       被切换 ⇒ 迟到的旧响应整体丢弃（entries 不被覆盖、过期游标不写入），且
       新口径自己的响应照常落地。修前守卫只比 view：切范围后旧响应放行，
       「源A×article」的收藏列表覆盖进源B 视图并持久留存。 */
    await resetStore();
    store.setState({ activeViewFilter: 'starred', articlesCursor: {} });
    S.listPlan = { mode: 'defer' };
    const fix4Late = store.getState().reloadFilteredEntries('starred'); // 发起时范围=all
    await nTick(0);
    store.getState().selectFeed('11');  // 期间切到源B：它自己的筛选请求也进 defer 队列
    await nTick(0);
    checkNew('(fix-4) 竞态场景成立：两个筛选请求都在途（旧口径在前）', S.pendingList.length === 2);
    const fix4CursorBefore = JSON.stringify(store.getState().articlesCursor);
    S.pendingList[0].resolve([mkRow({ id: 9001, feed_id: 10, is_starred: true, title: '迟到的旧口径数据' })]);
    await fix4Late;
    await nTick(20);
    checkNew('(fix-4) 旧 scope 的迟到筛选响应被丢弃：entries 不被覆盖、过期游标不写入（修前只比 view 会放行）',
      !store.getState().entries.some((e) => e.id === '9001')
      && JSON.stringify(store.getState().articlesCursor) === fix4CursorBefore);
    /* 正向对照：新口径自己的响应照常落地（守卫没有锁死正常路径） */
    S.pendingList[1].resolve([mkRow({ id: 9002, feed_id: 11, is_starred: true, title: '源B 的筛选结果' })]);
    await nTick(20);
    checkNew('(fix-4) 新口径响应照常落地：源B 的收藏行进列表、游标写在新范围键上',
      store.getState().entries.some((e) => e.id === '9002')
      && store.getState().articlesCursor['article|11']?.loaded === 1);
    S.listPlan = null;
}
