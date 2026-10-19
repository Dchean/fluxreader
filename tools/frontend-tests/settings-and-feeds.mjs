// tools/frontend-tests/settings-and-feeds.mjs
// 领域模块：设置合并/校验与订阅源、分类增删
// OPT-016C 拆分自 tools/frontend-regression.mjs（旧行区间 1589-1706、2298-2310、2983-3036、3038-3063）；
// 断言名称/条件文本原样迁移，仅做路径深度适配（import.meta.url 与动态 import 深一层）与
// 共享可变状态的 S. 归属重写。数据所有权：共享假后端/夹具/记录归 harness（见 harness.mjs 头注），
// 本模块不 import 第二份 store；域内自带夹具（大行集等）仍在本模块内独立构造与复位。
export const id = 'settings-and-feeds';

export async function run(ctx) {
  const { S, store, checkNew, nTick, resetStore, bootFixture, mkRow, overrideGlobal, FOLDERS, FEEDS } = ctx;
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
  /* R5 机制：每笔载荷 =「已确认基线 + 本笔 patch」。空库（后端无存量）首写只带
     本笔键是既有语义的细化；此处先以完整快照建立基线，再断言「含新值且不丢
     基线键」（旧断言锁的「全量快照」已由「基线键保留」替代，判别力不变）。 */
  S.settingsRaw = JSON.stringify(store.getState().settings);
  await store.getState().bootstrapSettings();
  S.invokeCalls.length = 0;
  store.getState().updateSettings({ fontSize: 20, themeMode: 'light' });
  await nTick(0);   // api.setSetting 内部 await getInvoke()
  const kWrite = S.invokeCalls.find((c) => c.cmd === 'set_setting');
  const kPayload = kWrite ? JSON.parse(kWrite.args.value) : {};
  checkNew('(k) updateSettings 合并进内存并落库（含新值且不丢基线键）',
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
    await store.getState().addFeed('', 'https://example.com/rss.xml', '', 'inherit', 'inherit', 'inherit', false);
    const fix1Call = S.invokeCalls.find((c) => c.cmd === 'add_feed');
    checkNew('(fix-1) 空 catId 的 addFeed payload.folderId === null（后端 None→未分类兜底可达；修前为 0）',
      !!fix1Call && fix1Call.args.folderId === null && fix1Call.args.feedUrl === 'https://example.com/rss.xml');
    checkNew('(opt011-add) 新 UI 路径 addFeed 以三态 mode 为输入：summaryMode/translateMode 原样发出、旧 bool 传 null',
      !!fix1Call && fix1Call.args.summaryMode === 'inherit' && fix1Call.args.translateMode === 'inherit'
      && fix1Call.args.autoSummary === null && fix1Call.args.autoTranslate === null);
    /* 有数字 id 的路径不受影响 */
    S.invokeCalls.length = 0;
    await store.getState().addFeed('cat-2', 'https://example.com/b.xml', '', 'inherit', 'off', 'on', false);
    const fix1b = S.invokeCalls.find((c) => c.cmd === 'add_feed');
    checkNew('(fix-1) 数字 catId 照旧映射（cat-2 → folderId=2）',
      !!fix1b && fix1b.args.folderId === 2);
    checkNew('(opt011-add) 显式三态照常发出（off/on 不再是 bool 回显）',
      !!fix1b && fix1b.args.summaryMode === 'off' && fix1b.args.translateMode === 'on');

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

  /* ============================================================
     OPT-011（F13）：AI 开关三态单点与输入范围一致
     - 三态 matrix（mode × 分类默认 × 摘要/翻译）literal；
     - 原子索引：任何切换后 categories 与 feedIndex 当帧同值（旧实现只 set
       categories，selectFeedConfig 读 feedIndex → 切换不生效/分类是死设置）；
     - 失败 latest 守卫：迟到失败不得回滚更晚的成功写入；
     - 旧行缺 mode 按旧 bool 映射（升级不改变既有开关语义）。
     数据所有权：FOLDERS/FEEDS 是 harness 共享常量，本段就地改写后 finally 复原。
     ============================================================ */
  {
    const { normalizeFeedAiMode, resolveFeedAiMode } = await import('../../dist-test/types.js');
    const { selectFeedConfig } = await import('../../dist-test/store/selectors.js');

    /* ---------- (opt011-matrix) resolver 的三态 matrix literal ----------
       行=mode，列=分类默认；期望值逐格字面写出（不是从实现反向推导）。 */
    const matrix = [
      ['inherit', false, false],
      ['inherit', true, true],
      ['on', false, true],
      ['on', true, true],
      ['off', false, false],
      ['off', true, false],
    ];
    checkNew('(opt011-matrix) resolveFeedAiMode 三态×分类默认逐格 literal（显式关闭压过分类开启）',
      matrix.every(([mode, cat, expect]) => resolveFeedAiMode(mode, cat) === expect));
    checkNew('(opt011-matrix) 旧输入归一：缺 mode/非法 mode 按旧 bool 映射显式 on/off（绝不落 inherit）',
      normalizeFeedAiMode(undefined, true) === 'on'
      && normalizeFeedAiMode(null, false) === 'off'
      && normalizeFeedAiMode('bogus', false) === 'off'
      && normalizeFeedAiMode('bogus', true) === 'on'
      && normalizeFeedAiMode('inherit', false) === 'inherit');

    /* ---------- (opt011-atomic) 真实 store：旧索引与死分类的两段红测 ---------- */
    await bootFixture();
    const catBefore = JSON.stringify(FOLDERS.find((f) => f.id === 1));
    try {
      /* 前置 A（旧索引红测）：旧行没有 mode 字段（harness 夹具即旧后端行），
         feed10 bool=false → 归一为显式 off；分类默认关 → 有效值 false。 */
      const c0 = selectFeedConfig(store.getState(), '10');
      checkNew('(opt011-oldrow) 旧行缺 mode 按旧 bool 映射：bool=false → off（升级不开启）',
        c0.summaryMode === 'off' && c0.autoSummary === false);

      /* 前置 B（死分类红测）：分类默认开，但旧行仍是显式 off（旧 bool=false）
         ——旧语义下分类开关对「继承」无意义；此时有效值必须保持 false。 */
      FOLDERS.find((f) => f.id === 1).auto_summary = true;
      await store.getState().reloadFromBackend();
      const c1 = selectFeedConfig(store.getState(), '10');
      checkNew('(opt011-dead-cat) 分类默认开对显式 off 的旧行无效（显式选择优先，不以 OR 合并）',
        c1.autoSummary === false);

      /* 原子索引 + 继承：把源切到 inherit → 有效值当帧跟随分类默认（开）。
         旧实现只 set categories 不重建 feedIndex：这里两条断言都会红。 */
      store.getState().toggleFeedSummary('cat-1', '10', 'inherit');
      const idxFeed = store.getState().feedIndex.get('10')?.feed;
      const c2 = selectFeedConfig(store.getState(), '10');
      checkNew('(opt011-atomic) 切换后 feedIndex 与 categories 当帧同值（旧实现只 set categories → index 是旧引用）',
        idxFeed?.summaryMode === 'inherit'
        && store.getState().categories.find((c) => c.id === 'cat-1')?.feeds.find((f) => f.id === '10')?.summaryMode === 'inherit');
      checkNew('(opt011-inherit) inherit × 分类开 → 有效摘要开启（分类默认真正生效，不再是死设置）',
        c2.autoSummary === true && c2.summaryMode === 'inherit');
      checkNew('(opt011-atomic) 同帧翻译开关未被连带改动（摘要与翻译独立三态）',
        c2.autoTranslate === false && c2.translateMode === 'off');

      /* 显式关闭压过分类开启（不以 OR 合并造成无法关闭） */
      store.getState().toggleFeedSummary('cat-1', '10', 'off');
      await nTick(20);
      const c3 = selectFeedConfig(store.getState(), '10');
      checkNew('(opt011-explicit-off) 分类开 + 源显式 off → 有效摘要关闭（显式关闭压过分类）',
        c3.autoSummary === false && c3.summaryMode === 'off');
      const offWrite = S.invokeCalls.filter((c) => c.cmd === 'set_feed_ai_modes').at(-1);
      checkNew('(opt011-explicit-off) 三态切换经新命令落库：只带被改字段（summaryMode=off，translateMode=null）',
        offWrite?.args.summaryMode === 'off' && offWrite?.args.translateMode === null);

      /* 分类切换同样必须原子重建索引：分类关 → inherit 源有效值当帧转 false */
      store.getState().toggleFeedSummary('cat-1', '10', 'inherit');
      store.getState().toggleCatSummary('cat-1', false);
      const c4 = selectFeedConfig(store.getState(), '10');
      checkNew('(opt011-atomic) 分类默认切换当帧可见：inherit 源有效值随分类关而关（feedIndex.cat 同一份）',
        c4.autoSummary === false
        && store.getState().feedIndex.get('10')?.cat.autoSummary === false);
      /* 等上面两笔在途 IPC 落定，再安装受控假后端——否则它们会混进 pending 队列 */
      await nTick(20);

      /* ---------- (opt011-latest) 失败 latest 守卫（per-键队列语义） ---------- */
      const pendingWrites = [];
      const restoreInvoke = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
        S.invokeCalls.push({ cmd, args });
        if (cmd === 'set_feed_ai_modes') {
          return new Promise((resolve, reject) => pendingWrites.push({ args, resolve, reject }));
        }
        return Promise.resolve(null);
      });
      try {
        /* 场景 A：同键两笔——第一笔在途、第二笔排队；第一笔失败不得回滚第二笔
           （守卫按序号跳过旧失败的回滚，队列在前一笔落定后继续发第二笔）。 */
        store.getState().toggleFeedSummary('cat-1', '10', 'on');
        await nTick(20);
        store.getState().toggleFeedSummary('cat-1', '10', 'off');
        await nTick(20);
        checkNew('(opt011-latest) per-键队列：同字段后一笔在前一笔落定前不发出，乐观值当帧已是最后意图',
          pendingWrites.length === 1
          && selectFeedConfig(store.getState(), '10').summaryMode === 'off');
        pendingWrites[0].reject({ message: '注入：旧写失败' });
        await nTick(40);
        checkNew('(opt011-latest) 旧写失败不回滚更晚的乐观写入（守卫跳过回滚），提示如实说明保持现状',
          selectFeedConfig(store.getState(), '10').summaryMode === 'off'
          && store.getState().toasts.at(-1)?.text.includes('较新的修改'));
        checkNew('(opt011-latest) 队列继续：旧写落定后第二笔才发出，且只带单字段 off',
          pendingWrites.length === 2 && pendingWrites[1].args?.summaryMode === 'off'
          && pendingWrites[1].args?.translateMode === null);
        pendingWrites[1].resolve(null);
        await nTick(20);
        checkNew('(opt011-latest) 第二笔成功：最终值=最后意图 off（不被旧失败踩回）',
          selectFeedConfig(store.getState(), '10').summaryMode === 'off');

        /* 场景 B：最新一笔自己失败 → 回滚到本笔乐观写入前的值并提示 */
        store.getState().toggleFeedSummary('cat-1', '10', 'inherit');
        await nTick(20);
        const beforeRollback = selectFeedConfig(store.getState(), '10').summaryMode;
        pendingWrites.at(-1).reject({ message: '注入：最新失败' });
        await nTick(20);
        const c6 = selectFeedConfig(store.getState(), '10');
        checkNew('(opt011-latest) 最新一笔失败回滚到写入前值（inherit 回 off），提示已恢复',
          c6.summaryMode === 'off' && c6.summaryMode !== beforeRollback
          && store.getState().toasts.at(-1)?.text.includes('已恢复原值'));
      } finally {
        restoreInvoke();
      }

      /* ---------- (opt011-edit) editFeed 以 mode 提交（不再 bool 回显覆盖） ---------- */
      S.invokeCalls.length = 0;
      const curModeNow = selectFeedConfig(store.getState(), '10').summaryMode;
      store.getState().editFeed('10', { title: '', catId: 'cat-1', layout: 'inherit', summaryMode: curModeNow, translateMode: 'on' });
      await nTick(20);
      const editCall = S.invokeCalls.find((c) => c.cmd === 'update_feed');
      checkNew('(opt011-edit) editFeed 只提交变化的 mode：summary 未变则省略，translate 变更为 on',
        editCall?.args.summaryMode === undefined && editCall?.args.translateMode === 'on'
        && editCall?.args.autoSummary === undefined && editCall?.args.autoTranslate === undefined);
    } finally {
      /* 复原共享夹具（FOLDERS 是跨域常量，就地改写必须还原） */
      Object.assign(FOLDERS.find((f) => f.id === 1), JSON.parse(catBefore));
      await resetStore();
    }

    /* ============================================================
       OPT-011（review 修复）：分类 AI 单字段 patch + per-字段独立队列
       缺陷：toggleCatSummary/Translate 各把「另一字段的当前值」随行快照整对
       写库；A(true,false) 迟到于 B(true,true) 时会把 B 的新写入用旧值回退。
       修后：IPC Option 单字段 patch（只带被改字段）+ per-键队列（同字段串行、
       跨字段独立）——乱序到达不互相覆盖，失败 latest 守卫保留。
       ============================================================ */
    {
      await bootFixture();
      S.invokeCalls.length = 0;
      const catPending = [];
      const restoreInvoke = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
        S.invokeCalls.push({ cmd, args });
        if (cmd === 'set_folder_ai_flags') {
          return new Promise((resolve, reject) => catPending.push({ args, resolve, reject }));
        }
        return Promise.resolve(null);
      });
      try {
        /* 两字段（不同键）分别切换：各自立即发出单字段 patch（独立队列，互不阻塞） */
        store.getState().toggleCatSummary('cat-1', true);
        await nTick(20);
        store.getState().toggleCatTranslate('cat-1', true);
        await nTick(20);
        const catCalls = catPending.map((p) => p.args);
        checkNew('(opt011-cat-patch) 分类切换只传当前字段：summary 笔不含 translate、translate 笔不含 summary（不再整对快照）',
          catCalls.length === 2
          && catCalls[0].summary === true && !('translate' in catCalls[0])
          && catCalls[1].translate === true && !('summary' in catCalls[1]));
        checkNew('(opt011-cat-queue) 两字段独立队列：不同字段的请求互不阻塞（都已在途）', catPending.length === 2);

        /* 乱序完成不互相覆盖：先放行更晚发出的 translate，再放行 summary */
        catPending[1].resolve(null);
        await nTick(20);
        catPending[0].resolve(null);
        await nTick(20);
        const catAfter = store.getState().feedIndex.get('10')?.cat;
        checkNew('(opt011-cat-queue) 乱序落定后两字段最终值均为各自最新意图（true/true，互不覆盖）',
          catAfter?.autoSummary === true && catAfter?.autoTranslate === true);

        /* 同字段串行：summary 在途时再切 summary，第二笔必须等前一笔落定才发出 */
        store.getState().toggleCatSummary('cat-1', false);
        await nTick(20);
        const inFlight = catPending.length;
        store.getState().toggleCatSummary('cat-1', true);
        await nTick(20);
        checkNew('(opt011-cat-queue) 同字段串行：前一笔未落定，后一笔不发出（乐观值已是最后意图）',
          catPending.length === inFlight
          && store.getState().feedIndex.get('10')?.cat.autoSummary === true);
        catPending[inFlight - 1].resolve(null);
        await nTick(20);
        checkNew('(opt011-cat-queue) 前一笔落定后，后一笔才发出（单字段 true，不带另一字段）',
          catPending.length === inFlight + 1
          && catPending.at(-1).args.summary === true && !('translate' in catPending.at(-1).args));
        /* 失败 latest 守卫保留：最新一笔失败 → 回滚到写入前值（false）并提示 */
        catPending.at(-1).reject({ message: '注入：分类最新失败' });
        await nTick(20);
        checkNew('(opt011-cat-queue) 失败 latest 守卫保留：最新失败回滚到前值并提示已恢复',
          store.getState().feedIndex.get('10')?.cat.autoSummary === false
          && store.getState().toasts.at(-1)?.text.includes('已恢复原值'));
      } finally {
        restoreInvoke();
        await resetStore();
      }
    }

    /* ============================================================
       OPT-011 R2（主控定向复核 2026-10-10 返工）：失败恢复到最近一次可确认值
       缺陷：回滚目标取「本笔乐观写入前的值」，可能是另一笔未落定的乐观值——
       false/off → 乐观 true/on → 最后意图 false/off 两笔均失败时，UI 停在
       true/on（从未持久化），源的有效自动摘要被带偏为 true。修后：per-键确认
       基线（链起点=发起前的持久化值；成功落定推进基线；未落定数归零即清理）——
       连续失败回滚到链起点值，前成功后失败回滚到成功笔值，链不跨 reload 存活。
       夹具基线：cat-1 autoSummary/autoTranslate 双 false；feed 10 旧行
       bool=false → 归一 off/off。临时 __INVOKE__ 覆盖经 finally 恢复。
       ============================================================ */
    {
      await bootFixture();
      const { reconcileCategories } = await import('../../dist-test/store/internals.js');
      const catBefore2 = JSON.stringify(FOLDERS.find((f) => f.id === 1));
      const prevInvoke = globalThis.__INVOKE__;
      const be = { pending: [] };
      const restoreInvoke = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
        /* 只接管两条 AI 写入命令（挂起供受控失败/成功），其余回落主假后端
           （reload/feed_counts 等仍真实执行） */
        if (cmd === 'set_feed_ai_modes' || cmd === 'set_folder_ai_flags') {
          S.invokeCalls.push({ cmd, args });
          return new Promise((resolve, reject) => be.pending.push({ cmd, args, resolve, reject }));
        }
        return prevInvoke(cmd, args);
      });
      const feedMode = (id) => selectFeedConfig(store.getState(), id);
      const catFlag = () => store.getState().feedIndex.get('10')?.cat;
      const sentFeedSummary = () => be.pending.filter((p) => p.cmd === 'set_feed_ai_modes' && p.args.summaryMode !== null);
      const sentFeedTranslate = () => be.pending.filter((p) => p.cmd === 'set_feed_ai_modes' && p.args.translateMode !== null);
      try {
        /* ---------- A1：源摘要 off → on → off，两笔均失败 → 回到 off ---------- */
        store.getState().toggleFeedSummary('cat-1', '10', 'on');
        await nTick(20);
        store.getState().toggleFeedSummary('cat-1', '10', 'off');
        await nTick(20);
        checkNew('(opt011-r2) 前置：同键第二笔在第一笔落定前排队（只发出一笔）',
          sentFeedSummary().length === 1);
        be.pending[0].reject({ message: '注入：摘要第一笔失败' });
        await nTick(40);
        checkNew('(opt011-r2) 前置：第一笔失败后队列继续发出第二笔',
          sentFeedSummary().length === 2);
        be.pending[1].reject({ message: '注入：摘要第二笔失败' });
        await nTick(40);
        const r1 = feedMode('10');
        checkNew('(opt011-r2) 两笔均失败（源摘要）：回滚到链起点持久化值 off——不是第一笔的乐观 on',
          r1.summaryMode === 'off' && r1.autoSummary === false);
        checkNew('(opt011-r2) 两笔均失败：feedIndex 与 categories 原子收敛到同一确认值 off',
          store.getState().feedIndex.get('10')?.feed.summaryMode === 'off'
          && store.getState().categories.find((c) => c.id === 'cat-1')?.feeds.find((f) => f.id === '10')?.summaryMode === 'off');
        checkNew('(opt011-r2) 最后一笔失败提示「已恢复原值」（回滚确实发生）',
          store.getState().toasts.at(-1)?.text.includes('已恢复原值'));
        be.pending.length = 0;

        /* ---------- A2：分类摘要 false → true → false，两笔均失败 → 回到 false ---------- */
        store.getState().toggleCatSummary('cat-1', true);
        await nTick(20);
        store.getState().toggleCatSummary('cat-1', false);
        await nTick(20);
        checkNew('(opt011-r2) 前置：分类同键第二笔排队（只发出一笔）',
          be.pending.filter((p) => p.cmd === 'set_folder_ai_flags').length === 1);
        be.pending[0].reject({ message: '注入：分类摘要第一笔失败' });
        await nTick(40);
        checkNew('(opt011-r2) 前置：分类第二笔在第一笔失败后发出',
          be.pending.filter((p) => p.cmd === 'set_folder_ai_flags').length === 2);
        be.pending[1].reject({ message: '注入：分类摘要第二笔失败' });
        await nTick(40);
        checkNew('(opt011-r2) 两笔均失败（分类摘要）：回滚到持久化 false——不是第一笔的乐观 true',
          catFlag()?.autoSummary === false);
        be.pending.length = 0;

        /* ---------- B：前失败后成功（源翻译）——成功笔值保持 ---------- */
        store.getState().toggleFeedTranslate('cat-1', '10', 'on');
        await nTick(20);
        store.getState().toggleFeedTranslate('cat-1', '10', 'inherit');
        await nTick(20);
        checkNew('(opt011-r2) 前置：翻译同键第二笔排队（只发出一笔）', sentFeedTranslate().length === 1);
        be.pending[0].reject({ message: '注入：翻译第一笔失败' });
        await nTick(40);
        checkNew('(opt011-r2) 前置：第一笔失败后第二笔发出', sentFeedTranslate().length === 2);
        be.pending[1].resolve(null);
        await nTick(40);
        checkNew('(opt011-r2) 前失败后成功（源翻译）：成功笔 inherit 保持，旧失败不回滚新意图',
          feedMode('10').translateMode === 'inherit');
        be.pending.length = 0;

        /* ---------- C：前成功后失败（分类翻译）——回滚到成功确认值 ---------- */
        store.getState().toggleCatTranslate('cat-1', true);
        await nTick(20);
        checkNew('(opt011-r2) 前置：分类翻译第一笔已发出',
          be.pending.filter((p) => p.cmd === 'set_folder_ai_flags').length === 1);
        be.pending[0].resolve(null);
        await nTick(20);
        store.getState().toggleCatTranslate('cat-1', false);
        await nTick(20);
        checkNew('(opt011-r2) 前置：分类翻译第二笔发出',
          be.pending.filter((p) => p.cmd === 'set_folder_ai_flags').length === 2);
        be.pending[1].reject({ message: '注入：分类翻译第二笔失败' });
        await nTick(40);
        checkNew('(opt011-r2) 前成功后失败（分类翻译）：回滚到第一笔成功确认值 true',
          catFlag()?.autoTranslate === true);
        be.pending.length = 0;

        /* ---------- D：三笔混合 成功/失败/失败——回滚到成功笔值（R1 会停在中间乐观值） ----------
           off → on(A 成功) → off(B 失败) → inherit(C 失败)：R1 的 C 回滚目标是
           B 的乐观 off；R2 回滚到基线 on（A 已持久化）。 */
        store.getState().toggleFeedSummary('cat-1', '10', 'on');
        await nTick(20);
        store.getState().toggleFeedSummary('cat-1', '10', 'off');
        await nTick(20);
        store.getState().toggleFeedSummary('cat-1', '10', 'inherit');
        await nTick(20);
        checkNew('(opt011-r2) 前置：三笔同键串行（仅首笔在途）', sentFeedSummary().length === 1);
        be.pending[0].resolve(null);
        await nTick(40);
        checkNew('(opt011-r2) 前置：第二笔发出', sentFeedSummary().length === 2);
        be.pending[1].reject({ message: '注入：混合-B 失败' });
        await nTick(40);
        checkNew('(opt011-r2) 前置：第三笔发出', sentFeedSummary().length === 3);
        be.pending[2].reject({ message: '注入：混合-C 失败' });
        await nTick(40);
        checkNew('(opt011-r2) 三笔混合 成功/失败/失败：回滚到最近成功笔 on（非链起点 off、非 B/C 的乐观值）',
          feedMode('10').summaryMode === 'on');
        be.pending.length = 0;

        /* ---------- D2：三笔混合 失败/失败/成功——成功收尾不回滚 ---------- */
        store.getState().toggleCatSummary('cat-1', true);
        await nTick(20);
        store.getState().toggleCatSummary('cat-1', false);
        await nTick(20);
        store.getState().toggleCatSummary('cat-1', true);
        await nTick(20);
        be.pending[0].reject({ message: '注入：混合2-A 失败' });
        await nTick(40);
        be.pending[1].reject({ message: '注入：混合2-B 失败' });
        await nTick(40);
        be.pending[2].resolve(null);
        await nTick(40);
        checkNew('(opt011-r2) 三笔混合 失败/失败/成功：最终为成功笔 true（旧失败跳过不回滚）',
          catFlag()?.autoSummary === true);
        be.pending.length = 0;

        /* ---------- E：跨字段独立——一笔失败回滚不触碰另一字段已成功的值 ---------- */
        store.getState().toggleFeedSummary('cat-1', '10', 'off');
        await nTick(20);
        store.getState().toggleFeedTranslate('cat-1', '10', 'off');
        await nTick(20);
        checkNew('(opt011-r2) 跨字段独立：摘要与翻译不同键各自立即发出（不互相排队）',
          sentFeedSummary().length === 1 && sentFeedTranslate().length === 1);
        be.pending.find((p) => p.args.translateMode !== null).resolve(null);
        await nTick(20);
        be.pending.find((p) => p.args.summaryMode !== null).reject({ message: '注入：摘要失败' });
        await nTick(40);
        checkNew('(opt011-r2) 跨字段独立：摘要失败回滚自身基线 on；已成功的翻译 off 不被触碰',
          feedMode('10').summaryMode === 'on' && feedMode('10').translateMode === 'off');
        be.pending.length = 0;

        /* 反向：翻译失败回滚不触碰摘要已成功值 */
        store.getState().toggleFeedTranslate('cat-1', '10', 'on');
        await nTick(20);
        store.getState().toggleFeedSummary('cat-1', '10', 'inherit');
        await nTick(20);
        be.pending.find((p) => p.args.summaryMode !== null).resolve(null);
        await nTick(20);
        be.pending.find((p) => p.args.translateMode !== null).reject({ message: '注入：翻译失败' });
        await nTick(40);
        checkNew('(opt011-r2) 跨字段独立（反向）：翻译失败回滚自身基线 off；摘要成功值 inherit 不被触碰',
          feedMode('10').summaryMode === 'inherit' && feedMode('10').translateMode === 'off');
        be.pending.length = 0;

        /* ---------- F：链不跨 reload 存活——外部真值变化后重置基线 ---------- */
        FOLDERS.find((f) => f.id === 1).auto_summary = true;
        await store.getState().reloadFromBackend();
        checkNew('(opt011-r2) 前置：外部 reload 后分类默认 = true（新的可确认值）',
          catFlag()?.autoSummary === true);
        store.getState().toggleCatSummary('cat-1', false);
        await nTick(20);
        be.pending[0].reject({ message: '注入：reload 后失败' });
        await nTick(40);
        checkNew('(opt011-r2) 链未跨 reload 残留：失败回滚到 reload 后的确认值 true（旧基线 false 不复活）',
          catFlag()?.autoSummary === true);
        be.pending.length = 0;

        /* ---------- G：实体已删除——在途失败落定不重建实体 ---------- */
        store.getState().toggleFeedSummary('cat-1', '10', 'on');
        await nTick(20);
        checkNew('(opt011-r2) 前置：删除实体前写入在途', be.pending.length === 1);
        const sBefore = store.getState();
        store.setState(reconcileCategories(sBefore, sBefore.categories.map((c) => (
          { ...c, feeds: c.feeds.filter((f) => f.id !== '10') }
        ))));
        be.pending[0].reject({ message: '注入：实体删除后的失败' });
        await nTick(40);
        checkNew('(opt011-r2) 失败落定不重建已删除实体（apply 对缺失实体 no-op），提示如实',
          store.getState().feedIndex.get('10') === undefined
          && store.getState().toasts.at(-1)?.text.includes('未能保存'));
      } finally {
        restoreInvoke();
        Object.assign(FOLDERS.find((f) => f.id === 1), JSON.parse(catBefore2));
        await resetStore();
      }
    }

    /* ============================================================
       OPT-011 R3（独立 review 复现的在途 reload 缺口）：接受点对齐确认态
       缺陷时序（源/分类均成立）：确认 on/true → 写 off/false 挂起 → 真实
       bootstrap/reload 接受新的后端 off/false → 原请求 reject → 旧值被回滚
       踩进 store，有效自动摘要错误变 true。
       修后（现由 aiConfigWrites 协议承担；本块断言按协议语义保持）：接受点
       按读取凭据更新确认态——实体存在 → 确认值 = 接受值；实体缺失 → 脱链
       （失败不回滚、不声称已恢复，重建后按新值重新接链）；接受时保留未落定
       意图。队列不动（刷新不清空队列：在途原位、新点击排队不并行）。
       本块用**真实 bootstrapFromBackend/reloadFromBackend**驱动（委托非开关命令
       给主假后端），不直接改内部 map；FEEDS/FOLDERS 就地改写后 finally 复原。
       ============================================================ */
    {
      await bootFixture();
      const catBefore3 = JSON.stringify(FOLDERS.find((f) => f.id === 1));
      const feedBefore3 = JSON.stringify(FEEDS.find((f) => f.id === 10));
      const fIdx = FEEDS.findIndex((f) => f.id === 10);
      const prevInvoke = globalThis.__INVOKE__;
      const be = { pending: [] };
      const restoreInvoke = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
        if (cmd === 'set_feed_ai_modes' || cmd === 'set_folder_ai_flags') {
          S.invokeCalls.push({ cmd, args });
          return new Promise((resolve, reject) => be.pending.push({ cmd, args, resolve, reject }));
        }
        return prevInvoke(cmd, args);
      });
      const feedMode = (id) => selectFeedConfig(store.getState(), id);
      const catFlag = () => store.getState().feedIndex.get('10')?.cat;
      const fsent = () => be.pending.filter((p) => p.cmd === 'set_feed_ai_modes' && p.args.summaryMode !== null);
      const csent = () => be.pending.filter((p) => p.cmd === 'set_folder_ai_flags');
      try {
        /* ---------- R3-1：源 pending → 真实 bootstrapFromBackend → reject ---------- */
        FEEDS.find((f) => f.id === 10).auto_summary = true;
        await store.getState().bootstrapFromBackend();
        checkNew('(opt011-r3) 前置：真实 bootstrap 后源确认为 on',
          feedMode('10').summaryMode === 'on');
        store.getState().toggleFeedSummary('cat-1', '10', 'off');
        await nTick(20);
        checkNew('(opt011-r3) 前置：写 off 挂起（乐观可见，链基线为 on）',
          fsent().length === 1 && feedMode('10').summaryMode === 'off');
        FEEDS.find((f) => f.id === 10).auto_summary = false; // 后端真值已持久化为 off
        await store.getState().bootstrapFromBackend();
        checkNew('(opt011-r3) 刷新不清空队列：bootstrap 后旧写仍在途原位（未重发/未并行）',
          fsent().length === 1);
        be.pending[0].reject({ message: '注入：挂起写在接受后失败' });
        await nTick(40);
        checkNew('(opt011-r3) 源 pending→bootstrap→reject：保持新确认态 off（旧 base on 不得踩回）',
          feedMode('10').summaryMode === 'off' && feedMode('10').autoSummary === false);
        checkNew('(opt011-r3) 源：feedIndex 与 categories 同一确认值 off，提示已恢复原值',
          store.getState().feedIndex.get('10')?.feed.summaryMode === 'off'
          && store.getState().categories.find((c) => c.id === 'cat-1')?.feeds.find((f) => f.id === '10')?.summaryMode === 'off'
          && store.getState().toasts.at(-1)?.text.includes('已恢复原值'));
        be.pending.length = 0;

        /* ---------- R3-2：分类 pending → 真实 bootstrapFromBackend → reject ---------- */
        FOLDERS.find((f) => f.id === 1).auto_summary = true;
        await store.getState().bootstrapFromBackend();
        checkNew('(opt011-r3) 前置：真实 bootstrap 后分类确认为 true', catFlag()?.autoSummary === true);
        store.getState().toggleCatSummary('cat-1', false);
        await nTick(20);
        checkNew('(opt011-r3) 前置：分类写 false 挂起（乐观可见）',
          csent().length === 1 && catFlag()?.autoSummary === false);
        FOLDERS.find((f) => f.id === 1).auto_summary = false;
        await store.getState().bootstrapFromBackend();
        be.pending[0].reject({ message: '注入：分类挂起写在接受后失败' });
        await nTick(40);
        checkNew('(opt011-r3) 分类 pending→bootstrap→reject：保持新确认 false（旧 base true 不得踩回）',
          catFlag()?.autoSummary === false);
        be.pending.length = 0;

        /* ---------- R3-3：reload 路径 + 刷新窗口内新点击（守卫不破坏 wire 顺序） ---------- */
        FEEDS.find((f) => f.id === 10).auto_summary = true;
        await store.getState().reloadFromBackend();
        store.getState().toggleFeedSummary('cat-1', '10', 'off');   // A：挂起
        await nTick(20);
        FEEDS.find((f) => f.id === 10).auto_summary = false;
        await store.getState().reloadFromBackend();                  // 接受新后端 off
        checkNew('(opt011-r3) 前置：reload 接受新后端 off', feedMode('10').summaryMode === 'off');
        store.getState().toggleFeedSummary('cat-1', '10', 'on');     // B：刷新窗口内新点击
        await nTick(20);
        checkNew('(opt011-r3) 刷新窗口内新点击不得与在途旧写并行（B 未发出，仍排 A 后）',
          fsent().length === 1);
        checkNew('(opt011-r3) 新点击乐观值当帧可见（旧 chain cleanup 不破坏）',
          feedMode('10').summaryMode === 'on');
        be.pending[0].reject({ message: '注入：A 在接受后失败' });
        await nTick(40);
        checkNew('(opt011-r3) 旧失败跳过回滚（保持现状提示），新写按序发出且只带单字段 on',
          feedMode('10').summaryMode === 'on' && fsent().length === 2
          && be.pending[1].args.summaryMode === 'on'
          && store.getState().toasts.at(-1)?.text.includes('较新的修改'));
        be.pending[1].reject({ message: '注入：B 失败' });
        await nTick(40);
        checkNew('(opt011-r3) 新写失败回滚到接受后的确认基线 off（非旧链 on、非 B 乐观 on）',
          feedMode('10').summaryMode === 'off');
        be.pending.length = 0;

        /* ---------- R3-4：实体缺失（脱链）→ reject 不重建；重建后旧回调不写回旧值 ---------- */
        FEEDS.find((f) => f.id === 10).auto_summary = true;
        await store.getState().reloadFromBackend();
        checkNew('(opt011-r3) 前置：重建场景起点确认为 on', feedMode('10').summaryMode === 'on');
        store.getState().toggleFeedSummary('cat-1', '10', 'off');   // A：挂起（base on）
        await nTick(20);
        const removed = FEEDS.splice(fIdx, 1)[0];                    // 后端移除该源
        await store.getState().reloadFromBackend();
        checkNew('(opt011-r3) 前置：移除后 reload 接受无该源（链仍在途）',
          store.getState().feedIndex.get('10') === undefined && fsent().length === 1);
        be.pending[0].reject({ message: '注入：实体移除后的失败' });
        await nTick(40);
        checkNew('(opt011-r3) 实体缺失后 reject：不重建实体，提示不声称已恢复',
          store.getState().feedIndex.get('10') === undefined
          && store.getState().toasts.at(-1)?.text.includes('未能保存')
          && !store.getState().toasts.at(-1)?.text.includes('已恢复原值'));
        be.pending.length = 0;

        removed.auto_summary = false;
        FEEDS.splice(fIdx, 0, removed);                              // 后端重建（off）
        await store.getState().reloadFromBackend();
        store.getState().toggleFeedSummary('cat-1', '10', 'on');     // B：挂起（base off）
        await nTick(20);
        FEEDS.splice(FEEDS.findIndex((f) => f.id === 10), 1);        // 再次移除
        await store.getState().reloadFromBackend();
        removed.auto_summary = true;
        FEEDS.splice(fIdx, 0, removed);                              // 重建（on）
        await store.getState().reloadFromBackend();
        checkNew('(opt011-r3) 前置：重建接受后 store 确认为 on（实体已回归）',
          feedMode('10').summaryMode === 'on');
        store.getState().toggleFeedSummary('cat-1', '10', 'off');    // C：重建后的新点击（排队）
        await nTick(20);
        be.pending[0].reject({ message: '注入：旧挂起写失败' });
        await nTick(40);
        be.pending[1].reject({ message: '注入：重建后新写失败' });
        await nTick(40);
        checkNew('(opt011-r3) 重建后旧失败不写回旧值：最新失败回滚到重建接受值 on（非旧 base off、非乐观 off）',
          feedMode('10').summaryMode === 'on');
        be.pending.length = 0;
      } finally {
        restoreInvoke();
        Object.assign(FOLDERS.find((f) => f.id === 1), JSON.parse(catBefore3));
        const pos = FEEDS.findIndex((f) => f.id === 10);
        if (pos >= 0) FEEDS.splice(pos, 1);
        FEEDS.splice(fIdx, 0, JSON.parse(feedBefore3));
        await resetStore();
      }
    }

    /* ============================================================
       OPT-011 R4：写入 × 快照对账协议（独立复审两个失败时序 + 边界）
       修复前提（R3 复审复现）：① A 成功后旧 feeds 快照晚接受 → B 失败回错值；
       ② 读取在途期间 B 新点击被旧快照覆盖且 B 最终成功不恢复。
       本块假后端：独立持久态 P（folders/feeds 深拷贝，不碰共享夹具）、读取返回
       **冻结副本**（请求发出时快照、放行时刻由用例控制）、写提交（commit*）与
       写回应（resolve/reject）分开控制；全部用可控 Promise，无随机 sleep。
       ============================================================ */
    {
      await bootFixture();
      const prevInvoke = globalThis.__INVOKE__;
      const P = {
        folders: JSON.parse(JSON.stringify(FOLDERS)),
        feeds: JSON.parse(JSON.stringify(FEEDS)),
      };
      const be = { reads: [], writes: [] };
      const restoreInvoke = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
        if (cmd === 'list_folders') {
          return new Promise((resolve, reject) => be.reads.push({ cmd, resolve, reject, snapshot: JSON.parse(JSON.stringify(P.folders)) }));
        }
        if (cmd === 'list_feeds') {
          return new Promise((resolve, reject) => be.reads.push({ cmd, resolve, reject, snapshot: JSON.parse(JSON.stringify(P.feeds)) }));
        }
        if (cmd === 'set_feed_ai_modes' || cmd === 'set_folder_ai_flags') {
          S.invokeCalls.push({ cmd, args });
          return new Promise((resolve, reject) => be.writes.push({ cmd, args, resolve, reject }));
        }
        return prevInvoke(cmd, args);
      });
      const feedMode = () => selectFeedConfig(store.getState(), '10').summaryMode;
      const catFlag = () => store.getState().feedIndex.get('10')?.cat.autoSummary;
      const feedWrites = () => be.writes.filter((w) => w.cmd === 'set_feed_ai_modes');
      const flushNewest = (n) => { for (const r of be.reads.splice(-n)) r.resolve(r.snapshot); };
      const commitFeed = (mode) => { P.feeds.find((x) => x.id === 10).summary_mode = mode; };
      const commitCat = (v) => { P.folders.find((x) => x.id === 1).auto_summary = v; };
      const controlledReload = async () => {
        const p = store.getState().reloadFromBackend();
        await nTick(20);
        flushNewest(be.reads.length);
        await p;
        await nTick(20);
      };
      try {
        /* ---------- R4-1 场景①（源）：A 成功后旧快照晚接受，B 失败不得回错值 ---------- */
        store.getState().toggleFeedSummary('cat-1', '10', 'on');   // A
        await nTick(20);
        store.getState().toggleFeedSummary('cat-1', '10', 'off');  // B（排队）
        await nTick(20);
        checkNew('(opt011-r4) 场景①（源）前置：A 在途、B 排队（只发一笔）', feedWrites().length === 1);
        const reload1 = store.getState().reloadFromBackend();      // 读取在 A 提交前冻结（off）
        await nTick(20);
        checkNew('(opt011-r4) 场景①（源）前置：reload 的 folders/feeds 已发出并冻结副本', be.reads.length === 2);
        commitFeed('on');
        be.writes[0].resolve(null);                                // A 成功持久化 on
        await nTick(40);
        checkNew('(opt011-r4) 场景①（源）前置：A 成功后 B 才发出（wire 顺序保持）', feedWrites().length === 2);
        flushNewest(be.reads.length);
        await reload1;
        await nTick(20);
        checkNew('(opt011-r4) 场景①（源）：接受时保留 B 的未落定意图 off（快照 off 不越权）', feedMode() === 'off');
        be.writes[1].reject({ message: '注入：B 失败' });
        await nTick(40);
        checkNew('(opt011-r4) 场景①（源）：旧快照晚接受 + B 失败 → 恢复 A 的成功确认值 on（UI/确认态与后端一致）',
          feedMode() === 'on' && selectFeedConfig(store.getState(), '10').autoSummary === true);
        checkNew('(opt011-r4) 场景①（源）：提示已恢复原值（恢复的是持久化值）',
          store.getState().toasts.at(-1)?.text.includes('已恢复原值'));
        be.writes.length = 0;

        /* ---------- R4-2 场景②（源）：读取在途点击 B，A 失败/B 成功 ---------- */
        commitFeed('on');
        await controlledReload();
        checkNew('(opt011-r4) 场景②（源）前置：受控 reload 后确认为 on', feedMode() === 'on');
        store.getState().toggleFeedSummary('cat-1', '10', 'off');  // A
        await nTick(20);
        commitFeed('off');                                         // 后端已提交 off（回应挂起）
        checkNew('(opt011-r4) 场景②（源）前置：A 在途（提交与回应分离）', feedWrites().length === 1);
        const reload2 = store.getState().reloadFromBackend();      // 读取冻结 off，接受暂缓
        await nTick(20);
        store.getState().toggleFeedSummary('cat-1', '10', 'on');   // B（刷新途中点击）
        await nTick(20);
        checkNew('(opt011-r4) 场景②（源）前置：B 排队未发出（A 仍在途，不并行）', feedWrites().length === 1);
        flushNewest(be.reads.length);
        await reload2;
        await nTick(20);
        checkNew('(opt011-r4) 场景②（源）：接受 off 快照不得吞掉 B 的未落定意图（可视值保持 on）',
          feedMode() === 'on');
        be.writes[0].reject({ message: '注入：A 失败' });
        await nTick(40);
        checkNew('(opt011-r4) 场景②（源）：旧笔失败只提示保持现状（不回滚更晚意图）',
          store.getState().toasts.at(-1)?.text.includes('较新的修改'));
        be.writes[1].resolve(null);                                // B 成功（后端已 on）
        await nTick(40);
        checkNew('(opt011-r4) 场景②（源）：B 成功后 UI 与真实成功结果一致（on）',
          feedMode() === 'on' && selectFeedConfig(store.getState(), '10').autoSummary === true);
        be.writes.length = 0;

        /* ---------- R4-3 场景①（分类） ---------- */
        store.getState().toggleCatSummary('cat-1', true);          // A
        await nTick(20);
        store.getState().toggleCatSummary('cat-1', false);         // B
        await nTick(20);
        checkNew('(opt011-r4) 场景①（分类）前置：A 在途、B 排队',
          be.writes.filter((w) => w.cmd === 'set_folder_ai_flags').length === 1);
        const reload3 = store.getState().reloadFromBackend();
        await nTick(20);
        commitCat(true);
        be.writes[0].resolve(null);
        await nTick(40);
        flushNewest(be.reads.length);
        await reload3;
        await nTick(20);
        be.writes[1].reject({ message: '注入：分类 B 失败' });
        await nTick(40);
        checkNew('(opt011-r4) 场景①（分类）：旧快照晚接受 + B 失败 → 恢复 A 的成功确认值 true',
          catFlag() === true);
        be.writes.length = 0;

        /* ---------- R4-4 场景②（分类） ---------- */
        commitCat(true);
        await controlledReload();
        checkNew('(opt011-r4) 场景②（分类）前置：受控 reload 后确认为 true', catFlag() === true);
        store.getState().toggleCatSummary('cat-1', false);         // A
        await nTick(20);
        commitCat(false);
        const reload4 = store.getState().reloadFromBackend();
        await nTick(20);
        store.getState().toggleCatSummary('cat-1', true);          // B（刷新途中点击）
        await nTick(20);
        flushNewest(be.reads.length);
        await reload4;
        await nTick(20);
        checkNew('(opt011-r4) 场景②（分类）：接受 false 快照不得吞掉 B 的意图（保持 true）',
          catFlag() === true);
        be.writes[0].reject({ message: '注入：分类 A 失败' });
        await nTick(40);
        be.writes[1].resolve(null);
        await nTick(40);
        checkNew('(opt011-r4) 场景②（分类）：B 成功后 UI 与真实成功结果一致（true）', catFlag() === true);
        be.writes.length = 0;

        /* ---------- R4-5 边界：较早读取晚到（查询代际淘汰，不覆盖较新接受的读取） ---------- */
        commitFeed('off');
        await controlledReload();
        const r1 = store.getState().reloadFromBackend();           // 旧读取：冻结 off
        await nTick(20);
        commitFeed('on');
        const r2 = store.getState().reloadFromBackend();           // 新读取：冻结 on（代际已推进）
        await nTick(20);
        flushNewest(2);
        await r2;
        await nTick(20);
        checkNew('(opt011-r4) 边界·较早读取晚到前置：较新读取接受后 = on', feedMode() === 'on');
        flushNewest(2);
        await r1;
        await nTick(20);
        checkNew('(opt011-r4) 边界·较早读取晚到：被淘汰的旧读取不得覆盖已接受的较新读取（仍 on）',
          feedMode() === 'on');

        /* ---------- R4-6 边界：成功回应迟到（读取接受期间挂起意图，成功后推进确认态） ---------- */
        commitFeed('off');
        await controlledReload();
        store.getState().toggleFeedSummary('cat-1', '10', 'on');   // A（回应挂起）
        await nTick(20);
        const reload6 = store.getState().reloadFromBackend();      // 读取冻结 off
        await nTick(20);
        flushNewest(be.reads.length);
        await reload6;
        await nTick(20);
        checkNew('(opt011-r4) 边界·成功回应迟到：接受期间保留挂起意图（on）', feedMode() === 'on');
        commitFeed('on');
        be.writes[0].resolve(null);                                // 成功回应迟到
        await nTick(40);
        checkNew('(opt011-r4) 边界·成功回应迟到：确认态推进到成功值，UI 与提交一致（on）',
          feedMode() === 'on' && selectFeedConfig(store.getState(), '10').autoSummary === true);
        be.writes.length = 0;

        /* ---------- R4-7 边界：读取失败结束凭据（不改状态、不悬挂后续写入） ---------- */
        const beforeFail = feedMode();
        const failP = store.getState().reloadFromBackend().catch(() => null);
        await nTick(20);
        be.reads.find((r) => r.cmd === 'list_feeds').reject({ message: '注入：读取失败' });
        be.reads.splice(0).forEach((r) => r.resolve(r.snapshot));
        await failP;
        await nTick(20);
        checkNew('(opt011-r4) 边界·读取失败：错误如实提示且 AI 值不被失败读取改动',
          feedMode() === beforeFail && store.getState().toasts.at(-1)?.text.includes('刷新失败'));
        store.getState().toggleFeedSummary('cat-1', '10', 'off');
        await nTick(20);
        be.writes[0].resolve(null);
        await nTick(40);
        checkNew('(opt011-r4) 边界·读取失败后：同键写入照常落定（队列/票据未悬挂）', feedMode() === 'off');
        be.writes.length = 0;
      } finally {
        restoreInvoke();
        /* 防御性清账：遗留的读取以冻结副本放行、遗留的写入以失败收尾（均为已挂
           处理器，不会产生未处理拒绝） */
        for (const r of be.reads.splice(0)) r.resolve(r.snapshot);
        for (const w of be.writes.splice(0)) w.reject({ message: 'teardown' });
        await resetStore();
      }
    }

    /* ============================================================
       OPT-011 R5：编辑中的 AI 字段接入同一写协议（独立复审两条全成功交错）
       缺陷：editFeed 直连 update_feed 绕过 per-键队列——A 已提交未回包、B(on)
       已排队时，编辑 C(off) 立即发出并成功，A 回包后旧 B 才发出，后端被 B 的
       旧排队写覆盖成 on（用户最后意图是 off）；② A(on) 迟到成功回包把编辑并
       回读后的 UI 从 off 改回 on。
       修后：含 AI 更改的编辑 = 一笔多键操作（只登记被改 AI 字段；完整
       update_feed 只发一次；请求排在涉及键队列尾之后；乐观落地一次原子索引
       更新；失败合并为一条提示）。
       本块假后端：独立持久态 P（folders/feeds 深拷贝）、读取返回冻结副本、
       写提交（改 P）与回应（resolve/reject）分开控制；全部受控 Promise。
       ============================================================ */
    {
      await bootFixture();
      const prevInvoke = globalThis.__INVOKE__;
      const P = {
        folders: JSON.parse(JSON.stringify(FOLDERS)),
        feeds: JSON.parse(JSON.stringify(FEEDS)),
      };
      const be = { reads: [], writes: [], updates: [] };
      const restoreInvoke = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
        if (cmd === 'list_folders') {
          return new Promise((resolve, reject) => be.reads.push({ cmd, resolve, reject, snapshot: JSON.parse(JSON.stringify(P.folders)) }));
        }
        if (cmd === 'list_feeds') {
          return new Promise((resolve, reject) => be.reads.push({ cmd, resolve, reject, snapshot: JSON.parse(JSON.stringify(P.feeds)) }));
        }
        if (cmd === 'set_feed_ai_modes' || cmd === 'set_folder_ai_flags') {
          S.invokeCalls.push({ cmd, args });
          return new Promise((resolve, reject) => be.writes.push({ cmd, args, resolve, reject }));
        }
        if (cmd === 'update_feed') {
          S.invokeCalls.push({ cmd, args });
          return new Promise((resolve, reject) => be.updates.push({ args, resolve, reject }));
        }
        return prevInvoke(cmd, args);
      });
      const feedCfg = () => selectFeedConfig(store.getState(), '10');
      const pFeed = () => P.feeds.find((f) => f.id === 10);
      const settleReads = async () => {
        for (const r of be.reads.splice(0)) r.resolve(r.snapshot);
        await nTick(20);
      };
      const controlledReload = async () => {
        const p = store.getState().reloadFromBackend();
        await nTick(20);
        for (const r of be.reads.splice(0)) r.resolve(r.snapshot);
        await p;
        await nTick(20);
      };
      try {
        pFeed().summary_mode = 'on';
        await controlledReload();
        checkNew('(opt011-r5) 前置：源确认为 on', feedCfg().summaryMode === 'on');

        /* ---------- R5-1 交错①：A 提交未回包、B(on) 排队；编辑 C(off) 排 B 后 ---------- */
        store.getState().toggleFeedSummary('cat-1', '10', 'off');        // A
        await nTick(20);
        checkNew('(opt011-r5) 交错①前置：A 在途（提交与回应分离）',
          be.writes.length === 1 && be.writes[0].args.summaryMode === 'off');
        pFeed().summary_mode = 'off';                                    // A 已提交（回包挂起）
        store.getState().toggleFeedSummary('cat-1', '10', 'on');         // B（排队）
        await nTick(20);
        checkNew('(opt011-r5) 交错①前置：B 排队未发出（同键串行）', be.writes.length === 1);
        store.getState().editFeed('10', { title: '', catId: 'cat-1', layout: 'inherit', summaryMode: 'off', translateMode: 'off' });
        await nTick(20);
        checkNew('(opt011-r5) 交错①：编辑不越过已排队 B——update_feed 未发出（修前直连绕过队列立即发出）',
          be.updates.length === 0 && be.writes.length === 1);
        checkNew('(opt011-r5) 交错①：编辑意图乐观可见（off 当帧，等待排队定序）',
          feedCfg().summaryMode === 'off');
        be.writes[0].resolve(null);                                      // A 成功回包
        await nTick(40);
        checkNew('(opt011-r5) 交错①前置：A 落定后 B 才发出（wire 顺序保持）', be.writes.length === 2);
        pFeed().summary_mode = 'on';                                     // B 提交 on
        be.writes[1].resolve(null);                                      // B 成功回包
        await nTick(40);
        checkNew('(opt011-r5) 交错①前置：B 落定后编辑 C 才发出（完整 update_feed 一笔、mode=off）',
          be.updates.length === 1 && be.updates[0].args.summaryMode === 'off');
        pFeed().summary_mode = 'off';                                    // C 提交 off（用户最后意图）
        be.updates[0].resolve(null);
        await nTick(40);
        checkNew('(opt011-r5) 交错①：最终后端与 UI/确认态都是用户最后意图 off（修前后端被旧排队 B 覆盖成 on）',
          pFeed().summary_mode === 'off' && feedCfg().summaryMode === 'off' && feedCfg().autoSummary === false);
        checkNew('(opt011-r5) 交错①：编辑成功提示如实（订阅源已更新）',
          store.getState().toasts.some((t) => t.text === '订阅源已更新'));
        await settleReads();                                             // 编辑成功后的全量刷新
        be.writes.length = 0;
        be.updates.length = 0;

        /* ---------- R5-2 交错②：A(on) 提交未回包 → 编辑 off；A 迟到成功不得改回 ---------- */
        pFeed().summary_mode = 'off';
        await controlledReload();
        checkNew('(opt011-r5) 交错②前置：源确认为 off', feedCfg().summaryMode === 'off');
        store.getState().toggleFeedSummary('cat-1', '10', 'on');         // A(on)
        await nTick(20);
        pFeed().summary_mode = 'on';                                     // A 已提交（回包挂起）
        store.getState().editFeed('10', { title: '', catId: 'cat-1', layout: 'inherit', summaryMode: 'off', translateMode: 'off' });
        await nTick(20);
        checkNew('(opt011-r5) 交错②：编辑 off 排在 A 之后未发出、乐观 off 当帧可见',
          be.updates.length === 0 && feedCfg().summaryMode === 'off');
        be.writes[0].resolve(null);                                      // A 迟到成功回包
        await nTick(40);
        checkNew('(opt011-r5) 交错②：编辑请求在 A 落定后发出（mode=off）',
          be.updates.length === 1 && be.updates[0].args.summaryMode === 'off');
        pFeed().summary_mode = 'off';                                    // 编辑提交 off
        be.updates[0].resolve(null);
        await nTick(40);
        checkNew('(opt011-r5) 交错②：A 的迟到成功不得把编辑的 off 改回 on——UI/确认态/后端一致 off',
          feedCfg().summaryMode === 'off' && pFeed().summary_mode === 'off');
        await settleReads();
        be.writes.length = 0;
        be.updates.length = 0;

        /* ---------- R5-3 双 AI 字段编辑：一次原子 UI 更新、一笔原 API、元数据保持 ---------- */
        pFeed().summary_mode = 'on';
        pFeed().translate_mode = 'off';
        await controlledReload();
        checkNew('(opt011-r5) 双字段编辑前置：摘要 on / 翻译 off',
          feedCfg().summaryMode === 'on' && feedCfg().translateMode === 'off');
        let emissions = 0;
        const unsub = store.subscribe(() => { emissions += 1; });
        store.getState().editFeed('10', { title: '新标题', catId: 'cat-1', layout: 'social', summaryMode: 'off', translateMode: 'on' });
        unsub();
        const cat10 = store.getState().categories.find((c) => c.id === 'cat-1')?.feeds.find((f) => f.id === '10');
        checkNew('(opt011-r5) 双字段编辑：乐观落地为一次原子索引更新（categories 与 feedIndex 同帧两字段同值）',
          emissions === 1
          && store.getState().feedIndex.get('10')?.feed.summaryMode === 'off'
          && store.getState().feedIndex.get('10')?.feed.translateMode === 'on'
          && cat10?.summaryMode === 'off' && cat10?.translateMode === 'on');
        await nTick(20);
        const upd = be.updates[0];
        checkNew('(opt011-r5) 双字段编辑：完整 update_feed 只发一次，元数据（标题/布局）保持且两 mode 同行',
          be.updates.length === 1 && upd?.args.title === '新标题' && upd?.args.layout === 'social'
          && upd?.args.summaryMode === 'off' && upd?.args.translateMode === 'on');
        checkNew('(opt011-r5) 双字段编辑：未改变的 AI 字段不回显（无旧 bool 参数），分类未移动不带 folderId',
          upd?.args.autoSummary === undefined && upd?.args.autoTranslate === undefined && upd?.args.folderId === undefined);
        pFeed().summary_mode = 'off'; pFeed().translate_mode = 'on';
        pFeed().title = '新标题'; pFeed().layout = 'social';
        upd.resolve(null);
        await nTick(40);
        checkNew('(opt011-r5) 双字段编辑：成功后两键确认态 = 成功值（off/on），UI 与后端一致',
          feedCfg().summaryMode === 'off' && feedCfg().translateMode === 'on');
        await settleReads();
        be.updates.length = 0;

        /* ---------- R5-3b 纯元数据编辑：不触碰 AI 键、不回显 AI 字段 ---------- */
        S.invokeCalls.length = 0;
        store.getState().editFeed('10', { title: '标题二', catId: 'cat-1', layout: 'social', summaryMode: feedCfg().summaryMode, translateMode: feedCfg().translateMode });
        await nTick(20);
        const metaUpd = be.updates[0];
        checkNew('(opt011-r5) 纯元数据编辑：AI 键不登记、update_feed 不带任何 mode/bool AI 字段',
          be.updates.length === 1 && metaUpd?.args.title === '标题二'
          && metaUpd?.args.summaryMode === undefined && metaUpd?.args.translateMode === undefined
          && metaUpd?.args.autoSummary === undefined && metaUpd?.args.autoTranslate === undefined);
        pFeed().title = '标题二';
        metaUpd.resolve(null);
        await nTick(40);
        await settleReads();
        be.updates.length = 0;

        /* ---------- R5-4 双字段编辑失败：合并一条提示、两键各自回滚到确认值 ---------- */
        store.setState({ toasts: [] });
        store.getState().editFeed('10', { title: '', catId: 'cat-1', layout: 'social', summaryMode: 'on', translateMode: 'off' });
        await nTick(20);
        checkNew('(opt011-r5) 双字段编辑失败前置：两键意图乐观可见（on/off）',
          feedCfg().summaryMode === 'on' && feedCfg().translateMode === 'off');
        be.updates[0].reject({ message: '注入：编辑失败' });
        await nTick(40);
        checkNew('(opt011-r5) 双字段编辑失败：两键回滚到最近确认值（off/on，非本笔乐观值）',
          feedCfg().summaryMode === 'off' && feedCfg().translateMode === 'on');
        const failToasts = store.getState().toasts.filter((t) => t.text.startsWith('保存失败'));
        checkNew('(opt011-r5) 双字段编辑失败：合并为一条失败提示（不重复 toast），文案如实说已恢复',
          failToasts.length === 1 && failToasts[0].text.includes('已恢复原值'));
        /* 失败不破坏队列：后续同键写照常发出（单字段 patch） */
        be.updates.length = 0;
        store.getState().toggleFeedSummary('cat-1', '10', 'on');
        await nTick(20);
        checkNew('(opt011-r5) 编辑失败后队列照常：后续写立即发出（单字段 patch，未粘滞）',
          be.writes.length === 1 && be.writes[0].args.summaryMode === 'on' && be.writes[0].args.translateMode === null);
        be.writes[0].resolve(null);
        await nTick(20);
        be.writes.length = 0;
      } finally {
        restoreInvoke();
        for (const r of be.reads.splice(0)) r.resolve(r.snapshot);
        for (const w of be.writes.splice(0)) w.resolve(null);
        for (const u of be.updates.splice(0)) u.reject({ message: 'teardown' });
        await resetStore();
      }
    }

    /* ---------- (opt011-interval) refreshInterval 统一 5–720 整数 ---------- */
    const beforeI = store.getState().settings.refreshInterval;
    store.getState().updateSettings({ refreshInterval: 720 });
    checkNew('(opt011-interval) 上限放宽到 720：边界值 720 合法可写', store.getState().settings.refreshInterval === 720);
    store.getState().updateSettings({ refreshInterval: 300 });
    checkNew('(opt011-interval) 中间值 300 合法（旧上限 120 会拒绝）', store.getState().settings.refreshInterval === 300);
    store.getState().updateSettings({ refreshInterval: 30.5 });
    checkNew('(opt011-interval) 小数 30.5 被写入路径拒绝（不落库，Rust 不会再静默回默认）',
      store.getState().settings.refreshInterval === 300);
    store.getState().updateSettings({ refreshInterval: 721 });
    store.getState().updateSettings({ refreshInterval: 4 });
    checkNew('(opt011-interval) 越界 4/721 拒绝', store.getState().settings.refreshInterval === 300);
    S.settingsRaw = JSON.stringify({ refreshInterval: 30.5 });
    await store.getState().bootstrapSettings();
    checkNew('(opt011-interval) 读回路径同样拒绝小数（写入/读回同一张整数校验表）',
      store.getState().settings.refreshInterval === 300);
    S.settingsRaw = JSON.stringify({ refreshInterval: 600 });
    await store.getState().bootstrapSettings();
    checkNew('(opt011-interval) 读回路径接受区间内整数（600）',
      store.getState().settings.refreshInterval === 600);
    store.getState().updateSettings({ refreshInterval: beforeI });
    S.settingsRaw = null;
  }
}
