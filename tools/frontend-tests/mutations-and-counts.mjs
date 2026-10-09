// tools/frontend-tests/mutations-and-counts.mjs
// 领域模块：已读/收藏变更、乐观回滚与未读计数一致性
// OPT-016C 拆分自 tools/frontend-regression.mjs（旧行区间 889-944、946-1053、1858-1923、1925-2090、2199-2228、2230-2242、3066-3307、3864-4032）；
// 断言名称/条件文本原样迁移，仅做路径深度适配（import.meta.url 与动态 import 深一层）与
// 共享可变状态的 S. 归属重写。数据所有权：共享假后端/夹具/记录归 harness（见 harness.mjs 头注），
// 本模块不 import 第二份 store；域内自带夹具（大行集等）仍在本模块内独立构造与复位。
export const id = 'mutations-and-counts';

export async function run(ctx) {
  const { S, store, checkNew, nTick, resetStore, bootFixture, NOW, iso, mkRow, countsFromRows, selectVisibleEntries, selectTreeCounts, selectViewCounts, bodyOf, overrideGlobal } = ctx;
  await ctx.useMainBackend();

  /* ============================================================
     (e) 已读 / 收藏切换与未读计数
     ============================================================ */
  await bootFixture();
  store.setState({ activeArticleId: '101' });
  S.invokeCalls.length = 0;
  store.getState().toggleCurrentReadStatus();
  await nTick(0);   // api.setRead 内部 await getInvoke()，落库是异步 fire-and-forget
  const eRead = store.getState();
  const eReadCall = S.invokeCalls.find((c) => c.cmd === 'set_read');
  checkNew('(e) 标已读：落库 read=true + 条目置位 + 该源未读 -1 + toast 文案正确',
    eReadCall?.args.id === 101 && eReadCall?.args.read === true
    && eRead.entries.find((a) => a.id === '101')?.isRead === true
    && eRead.feedCounts.get('10')?.unread === 2
    && eRead.toasts[eRead.toasts.length - 1]?.text === '已标为已读');
  store.getState().toggleCurrentReadStatus();
  await nTick(0);
  const eUnread = store.getState();
  const eUnreadCall = S.invokeCalls.filter((c) => c.cmd === 'set_read').pop();
  checkNew('(e) 再切回未读：落库 read=false + 未读计数回到 3 + toast 文案正确',
    eUnreadCall?.args.read === false
    && eUnread.entries.find((a) => a.id === '101')?.isRead === false
    && eUnread.feedCounts.get('10')?.unread === 3
    && eUnread.toasts[eUnread.toasts.length - 1]?.text === '已标为未读');
  const eToastCount = store.getState().toasts.length;
  store.getState().toggleCurrentStar();
  await nTick(0);
  const eStar = store.getState();
  const eStarCall = S.invokeCalls.find((c) => c.cmd === 'set_starred');
  checkNew('(e) 收藏切换：落库 starred=true + 收藏计数 +1 + 未读计数不受影响 + 不弹 toast',
    eStarCall?.args.id === 101 && eStarCall?.args.starred === true
    && eStar.entries.find((a) => a.id === '101')?.isStarred === true
    && eStar.feedCounts.get('10')?.starred === 2 && eStar.feedCounts.get('10')?.unread === 3
    && eStar.toasts.length === eToastCount);
  S.invokeCalls.length = 0;
  const eNoToast = store.getState().toasts.length;
  store.setState({ activeArticleId: null });
  store.getState().toggleCurrentReadStatus();
  store.getState().toggleCurrentStar();
  await nTick(0);
  checkNew('(e) 无选中文章：已读/收藏切换均为 no-op（无 IPC、无 toast、数据不变）',
    S.invokeCalls.length === 0 && store.getState().toasts.length === eNoToast
    && store.getState().entries.find((a) => a.id === '101')?.isRead === false);
  store.setState({ activeArticleId: '9999' });
  store.getState().toggleCurrentStar();
  await nTick(0);
  checkNew('(e) 选中 id 不在 entries 中时同样 no-op（防悬空 id 误写库）', S.invokeCalls.length === 0);
  store.setState({
    activeArticleId: '104',
    feedCounts: new Map([['12', { total: 2, unread: 0, starred: 0, today: 0 }]]),
  });
  store.getState().toggleCurrentReadStatus();
  await nTick(0);
  checkNew('(e) 后端计数已为 0 时标读不产生负数（未读计数夹取在 0）',
    store.getState().feedCounts.get('12')?.unread === 0
    && store.getState().entries.find((a) => a.id === '104')?.isRead === true);

  /* ============================================================
     (f) markAllRead / markCurrentViewAllRead 的范围语义
     ============================================================ */
  /* 【TASK-107 改动理由】计数断言改为「与后端对账」口径：假后端升级为忠实
     聚合（feedCountsImpl），markCurrentViewAllRead 成功后重取 feed_counts
     整体替换——期望值从「乐观按已加载推算」改为后端真值（本块行级数据：
     feed10 未读 101/103、feed11 未读 201、feed12 未读 104/105；starredOnly
     只标 103/201）。保护意图不变且更强：数字必须等于后端口径而非本地推算。 */
  await bootFixture();
  S.feedCountsImpl = countsFromRows;
  store.setState({ activeViewFilter: 'starred', timelineFilter: 'all', activeFeedFilter: 'all', openedReadIds: { '103': true } });
  S.invokeCalls.length = 0;
  store.getState().markCurrentViewAllRead();
  await nTick(0);   // api.markAllRead 内部 await getInvoke()，需让出微任务
  const f1 = store.getState();
  const fMark = S.invokeCalls.find((c) => c.cmd === 'mark_all_read');
  checkNew('(f) 收藏视图全部已读：范围参数 starredOnly=true，且只标该视图可见条目',
    fMark?.args.starredOnly === true && fMark?.args.feedId === null && fMark?.args.sinceMs === null
    && f1.entries.find((a) => a.id === '103')?.isRead === true
    && f1.entries.find((a) => a.id === '101')?.isRead === false
    && f1.entries.find((a) => a.id === '102')?.isRead === true);
  /* 本块 scope = all × article 布局 × starred 视图：被标读的只有 103（feed10）；
     feed11 是 social 布局源，201 两侧（前端可见集/后端范围）都不在范围内 → 计数 1 保持 */
  checkNew('(f) 全部已读后计数=后端口径（源A 2→1：仅标读的 103 扣减；源D 2、跨布局源B 1 不动）',
    f1.feedCounts.get('10')?.unread === 1 && f1.feedCounts.get('12')?.unread === 2
    && f1.feedCounts.get('11')?.unread === 1);
  checkNew('(f) 全部已读后清空「已读保留」快照（列表不再保留灰色卡片）',
    Object.keys(f1.openedReadIds).length === 0);
  checkNew('(f) 全部已读给出 toast 反馈', f1.toasts.some((t) => t.text === '已全部标为已读'));

  await bootFixture();
  S.feedCountsImpl = countsFromRows;
  store.setState({ activeViewFilter: 'all', timelineFilter: 'unread', activeFeedFilter: 'cat-1' });
  S.invokeCalls.length = 0;
  store.getState().markCurrentViewAllRead();
  await nTick(0);
  const fCatCall = S.invokeCalls.find((c) => c.cmd === 'mark_all_read');
  const fCat = store.getState();
  checkNew('(f) 分类范围 → folderId=数字、feedId=null（cat- 前缀不被当作源 id）',
    fCatCall?.args.folderId === 1 && fCatCall?.args.feedId === null);
  /* TASK-107：计数期望值改为后端对账口径（整个分类范围的未读 101/103/104/105
     都被标读：feed10 2→0、feed12 2→0）；分类外的 feed11（201）计数与读态均不受影响 */
  checkNew('(f) 分类范围只标该分类可见条目，分类外条目不受影响',
    fCat.entries.find((a) => a.id === '101')?.isRead === true
    && fCat.entries.find((a) => a.id === '103')?.isRead === true
    && fCat.entries.find((a) => a.id === '201')?.isRead === false
    && fCat.feedCounts.get('10')?.unread === 0 && fCat.feedCounts.get('12')?.unread === 0
    && fCat.feedCounts.get('11')?.unread === 1);

  await bootFixture();
  store.setState({ activeViewFilter: 'all', timelineFilter: 'unread', activeFeedFilter: '12' });
  S.invokeCalls.length = 0;
  store.getState().markCurrentViewAllRead();
  await nTick(0);
  const fFeedCall = S.invokeCalls.find((c) => c.cmd === 'mark_all_read');
  const fFeed = store.getState();
  checkNew('(f) 单源范围 → feedId=12（纯数字 id 提取，不截成 NaN）且只标该源条目',
    fFeedCall?.args.feedId === 12 && fFeedCall?.args.folderId === null
    && fFeed.entries.filter((a) => a.feedId === '12').every((a) => a.isRead)
    && fFeed.entries.find((a) => a.id === '101')?.isRead === false);

  /* 布局维度：后端范围必须与当前布局同口径——修前不带 layout，文章布局点一次
     会把社交/通知/播客/画廊布局的源一并标读并逐条推远端（审计 round-3 真机：
     可见 14 张、后端写入 74 条，覆盖 5 个布局）。 */
  await bootFixture();
  store.getState().selectLayout('social');
  store.setState({ activeViewFilter: 'all', timelineFilter: 'unread', openedReadIds: {} });
  S.invokeCalls.length = 0;
  store.getState().markCurrentViewAllRead();
  await nTick(0);
  const fLayoutCall = S.invokeCalls.find((c) => c.cmd === 'mark_all_read');
  checkNew('(f) 全部已读带当前布局：layout=social（缺此参数则跨布局误标并推远端）',
    fLayoutCall?.args.layout === 'social' && fLayoutCall?.args.feedId === null
    && fLayoutCall?.args.folderId === null);

  await bootFixture();
  store.setState({ dataMode: 'mock', activeViewFilter: 'all', timelineFilter: 'unread', activeFeedFilter: 'all' });
  S.invokeCalls.length = 0;
  store.getState().markCurrentViewAllRead();
  await nTick(0);
  checkNew('(f) mock 模式全部已读：本地生效但不发 IPC（无库可写，绝不伪造落库）',
    S.invokeCalls.filter((c) => c.cmd === 'mark_all_read').length === 0
    && store.getState().entries.find((a) => a.id === '101')?.isRead === true
    && store.getState().toasts.some((t) => t.text === '已全部标为已读'));

  await bootFixture();
  store.setState({ openedReadIds: { '999': true } });
  S.invokeCalls.length = 0;
  store.getState().markEntriesReadBulk(['101', '102', '103']);
  await nTick(0);
  /* AUDIT P3[F4]（TASK-084）：契约由「每个未读项一次 set_read」改为「整批一次
     set_read_bulk」。原断言（逐条 set_read）编码的是修前的行为，本卡按审计要求
     消除逐条 IPC，故此处**有意改写**并保留其原有意图（已读项不得重复写库）：
     ① 只发一次 set_read_bulk；
     ② 载荷只含未读项（已读的 102 不在其中）。 */
  checkNew('(f) 批量标读只发**一次** set_read_bulk IPC（修前是 N 次 set_read）',
    S.invokeCalls.filter((c) => c.cmd === 'set_read_bulk').length === 1
    && S.invokeCalls.filter((c) => c.cmd === 'set_read').length === 0);
  checkNew('(f) 批量标读只对未读项发 IPC（已读项不重复写库刷同步队列）',
    (S.invokeCalls.find((c) => c.cmd === 'set_read_bulk')?.args.ids ?? [])
      .slice().sort((a, b) => a - b).join(',') === '101,103');
  checkNew('(f) 批量标读的保留快照是「合并」而非替换（既有记录不丢）',
    store.getState().openedReadIds['999'] === true && store.getState().openedReadIds['101'] === true);
  checkNew('(f) 批量标读按源聚合未读减量（源A 标 2 条 → 3-2=1）',
    store.getState().feedCounts.get('10')?.unread === 1);
  S.invokeCalls.length = 0;
  store.getState().markEntriesReadBulk([]);
  checkNew('(f) 空 id 列表批量标读为 no-op（不发 IPC）', S.invokeCalls.length === 0);

  /* ---------- (p) TASK-067 N9/N10：交互落库与错误可见性 ---------- */
  /* 【TASK-107 改动理由】旧断言文案『全部已读未能保存，重启后可能回退』对应旧
     失败语义（本地保持已读假成功、重启回退）。新契约：失败即回滚（乐观读态/
     计数/已读保留快照全部还原），成功 toast 不再提前弹；断言升级为「回滚到位
     + 失败 toast 带重试 + 无假成功提示」，保护更强非弱化。 */
  await bootFixture();
  S.rejectCmds.add('mark_all_read');
  store.setState({ activeViewFilter: 'all', activeFeedFilter: '10', toasts: [] });
  const p1BeforeCounts = store.getState().feedCounts.get('10')?.unread;
  const p1BeforeOpened = Object.keys(store.getState().openedReadIds).length;
  store.getState().markCurrentViewAllRead();
  await nTick(10);
  const p1After = store.getState();
  checkNew('(p1) 全部已读失败必须可见且带重试入口（TASK-107：无「已全部标为已读」假成功提示）',
    p1After.toasts.some((t) => t.text.startsWith('全部已读保存失败') && t.action?.label === '重试')
    && !p1After.toasts.some((t) => t.text === '已全部标为已读'));
  checkNew('(p1) 失败回滚到位：已读态还原、未读计数还原、「已读保留」快照还原（TASK-107）',
    p1After.entries.filter((a) => a.feedId === '10' && a.isRead).length === 1 // 仅 102 本就已读
    && p1After.feedCounts.get('10')?.unread === p1BeforeCounts
    && Object.keys(p1After.openedReadIds).length === p1BeforeOpened);

  await bootFixture();
  S.rejectCmds.add('set_read');
  store.setState({ toasts: [], settings: { ...store.getState().settings, markReadOnOpen: true } });
  store.getState().selectArticle('101');
  await nTick(10);
  checkNew('(p2) 打开文章标读失败必须可见（修前静默，重启后回退未读）',
    store.getState().toasts.some((t) => t.text.startsWith('标读失败：')));

  /* (p2b) CF-04：卡片路径（SocialCard/NotifCard/GalleryCard/右键菜单共用的唯一入口）
     标读失败必须回滚本地乐观置位并可见——修前乐观置位后静默，重启回退未读。
     TASK-093 加强（审查 F4：本条修前代码下也通过，无判别力）：把「失败后不得
     残留乐观值 + 失败 toast」并入本条——修前（静默 catch / 回滚被去掉）下本条
     变红，乐观置位与回滚两侧各有判据。 */
  await bootFixture();
  S.rejectCmds.add('set_read');
  store.setState({ toasts: [] });
  const p2bBefore = store.getState().entries.find((e) => e.id === '101')?.isRead;
  store.getState().toggleEntryFlag('101', 'isRead');
  const p2bOptimistic = store.getState().entries.find((e) => e.id === '101')?.isRead === !p2bBefore;
  await nTick(20);
  checkNew('(p2b) 卡片路径乐观置位：点下去立即生效（未等落库），失败后不残留乐观值（TASK-093 加强）',
    p2bOptimistic
    && store.getState().entries.find((e) => e.id === '101')?.isRead === p2bBefore
    && store.getState().toasts.some((t) => t.text.startsWith('标读保存失败：')));
  await nTick(20);
  checkNew('(p2b) 卡片路径标读失败：回滚到点击前状态 + 失败 toast + 未读计数复原',
    store.getState().entries.find((e) => e.id === '101')?.isRead === p2bBefore
    && store.getState().feedCounts.get('10')?.unread === 3
    && store.getState().toasts.some((t) => t.text.startsWith('标读保存失败：')));

  /* (p2c) CF-03：搜索/命令面板锚定打开（anchorToArticle）的标读失败同样可见 */
  await bootFixture();
  S.rejectCmds.add('set_read');
  store.setState({ toasts: [], settings: { ...store.getState().settings, markReadOnOpen: true } });
  await store.getState().anchorToArticle('101');
  await nTick(20);
  checkNew('(p2c) 锚定打开标读失败必须可见（修前无声：本地已置读、库里没有）',
    store.getState().toasts.some((t) => t.text.startsWith('标读失败：')));

  await bootFixture();
  S.failReload = { message: 'db busy' };
  store.setState({ toasts: [] });
  await store.getState().reloadFromBackend().catch(() => {});
  checkNew('(p3) reloadFromBackend 失败必须可见（后台刷新/范围切换路径，修前静默）',
    store.getState().toasts.some((t) => t.text.startsWith('刷新失败：')));

  /* ============================================================
     (p3-f2 / p3-f3) TASK-093：乐观回滚收口（Batch 1/2 独立审查 F2/F3）
     - F2：回滚是「恢复点击前值」而非「再翻一次当前值」——连点两次、第一次
       失败第二次成功时，第一次迟到的失败不得把第二次已落库的新值踩回
       （已读 + 收藏各一条）；另补收藏单击失败回滚（审查 M2c：修后回归网
       对「收藏失败回滚」零保护）。
     - F3：阅读器 toggleCurrentReadStatus / toggleCurrentStar 与卡片同口径——
       成功提示只在落库成功后出现（P1-5 去假成功同口径），失败回滚且只出
       失败提示；翻译失败按「无半截未消毒产物即清 rawTranslatedIds」规则
       处理（清/留成对断言，Reader 与卡片路径各一组）。
     每条的修前变红实现（临时回退旧代码）与输出存 tmp/task-093/。
     ============================================================ */

  /* F2 已读连点：click1 落库失败（乐观置位 true）、click2 落库成功（false）。
     rejectWhen 只拒 set_read(read=true)——click1 的置位写被拒，click2 的写回
     成功；两次 toggle 在同一同步帧内完成（连点），click1 的迟到失败在其后才
     落地。修前（「再翻一次当前值」回滚）：click1 迟到的 catch 把 click2 已落库
     的 false 再翻回 true——UI=true / DB=false，本条红。 */
  await bootFixture();
  S.rejectWhen = (cmd, args) => cmd === 'set_read' && args.read === true;
  store.setState({ toasts: [] });
  const p3f2ReadBefore = store.getState().entries.find((e) => e.id === '101')?.isRead;
  store.getState().toggleEntryFlag('101', 'isRead');      // click1：乐观→true，set_read(true) 将失败
  store.getState().toggleEntryFlag('101', 'isRead');      // click2：乐观→false，set_read(false) 成功
  await nTick(30);
  checkNew('(p3-f2) 卡片已读连点两次第一次失败：最终 UI 与后端一致（第二次点击的值），迟到失败只提示不踩回',
    store.getState().entries.find((e) => e.id === '101')?.isRead === p3f2ReadBefore
    && store.getState().feedCounts.get('10')?.unread === 3
    && store.getState().toasts.some((t) => t.text.startsWith('标读保存失败：')));

  /* F2 收藏连点（set_starred 路径同形） */
  await bootFixture();
  S.rejectWhen = (cmd, args) => cmd === 'set_starred' && args.starred === true;
  store.setState({ toasts: [] });
  const p3f2StarBefore = store.getState().entries.find((e) => e.id === '101')?.isStarred;
  store.getState().toggleEntryFlag('101', 'isStarred');   // click1：乐观→true，set_starred(true) 将失败
  store.getState().toggleEntryFlag('101', 'isStarred');   // click2：乐观→false，set_starred(false) 成功
  await nTick(30);
  checkNew('(p3-f2) 卡片收藏连点两次第一次失败：最终 UI 与后端一致，迟到失败只提示不踩回',
    store.getState().entries.find((e) => e.id === '101')?.isStarred === p3f2StarBefore
    && store.getState().feedCounts.get('10')?.starred === 1
    && store.getState().toasts.some((t) => t.text.startsWith('收藏保存失败：')));

  /* M2c（审查：收藏失败回滚在修后回归网下全绿）：收藏单击失败必须回滚 + 计数复原 */
  await bootFixture();
  S.rejectCmds.add('set_starred');
  store.setState({ toasts: [] });
  const p3f2StarOrig = store.getState().entries.find((e) => e.id === '101')?.isStarred;
  store.getState().toggleEntryFlag('101', 'isStarred');
  await nTick(30);
  checkNew('(p3-f2) 卡片收藏单击失败：回滚到点击前值 + 失败 toast + 收藏计数复原（M2c 变红）',
    store.getState().entries.find((e) => e.id === '101')?.isStarred === p3f2StarOrig
    && store.getState().feedCounts.get('10')?.starred === 1
    && store.getState().toasts.some((t) => t.text.startsWith('收藏保存失败：')));

  /* F3 阅读器 toggleCurrentReadStatus：成功提示只在落库成功后；失败回滚且只出失败提示 */
  await bootFixture();
  S.rejectCmds.add('set_read');
  store.setState({ activeArticleId: '101', toasts: [] });
  store.getState().toggleCurrentReadStatus();
  checkNew('(p3-f3) 阅读器标读：乐观置位立即生效，但不提前弹「已标为已读」（修前先弹假成功）',
    store.getState().entries.find((a) => a.id === '101')?.isRead === true
    && !store.getState().toasts.some((t) => t.text === '已标为已读'));
  await nTick(30);
  checkNew('(p3-f3) 阅读器标读失败：回滚到点击前值，且只有失败提示（无成功提示残留）',
    store.getState().entries.find((a) => a.id === '101')?.isRead === false
    && store.getState().toasts.some((t) => t.text.startsWith('标读状态保存失败：'))
    && !store.getState().toasts.some((t) => t.text === '已标为已读'));

  await bootFixture();
  store.setState({ activeArticleId: '101', toasts: [] });
  store.getState().toggleCurrentReadStatus();
  await nTick(30);
  checkNew('(p3-f3) 阅读器标读成功：「已标为已读」提示在落库成功后出现（P1-5 同口径）',
    store.getState().entries.find((a) => a.id === '101')?.isRead === true
    && store.getState().toasts.some((t) => t.text === '已标为已读'));

  await bootFixture();
  S.rejectCmds.add('set_starred');
  store.setState({ activeArticleId: '103', toasts: [] });
  store.getState().toggleCurrentStar();
  await nTick(30);
  checkNew('(p3-f3) 阅读器收藏失败：回滚到点击前值 + 失败提示 + 收藏计数复原',
    store.getState().entries.find((a) => a.id === '103')?.isStarred === true
    && store.getState().feedCounts.get('10')?.starred === 1
    && store.getState().toasts.some((t) => t.text.startsWith('收藏状态保存失败：')));

  /* F3 阅读器翻译失败：rawTranslatedIds 按「无半截未消毒产物即清」规则处理（成对） */
  await bootFixture();
  S.aiTr = { deltas: [], error: '限流', reject: null, finish: true, holdIds: [] };
  store.setState({ toasts: [] });
  store.getState().selectArticle('201');
  await nTick(10);
  store.getState().toggleReaderTranslation();
  await nTick(20);
  checkNew('(p3-f3) Reader 翻译流内错误且无半截产物：rawTranslatedIds 清除（修前残留 → 消毒译文被按纯文本渲染）',
    store.getState().rawTranslatedIds['201'] === undefined
    && store.getState().translateErrors['201'] === '限流'
    && store.getState().isShowingTranslatedProse === false);

  await bootFixture();
  S.aiTr = { deltas: ['<b>半截'], error: '限流', reject: null, finish: true, holdIds: [] };
  store.setState((s) => ({
    toasts: [],
    entries: s.entries.map((a) => (a.id === '201' ? { ...a, content: '<p>详情</p>' } : a)),
  }));
  store.getState().selectArticle('201');
  await nTick(10);
  store.getState().toggleReaderTranslation();
  await nTick(20);
  checkNew('(p3-f3) Reader 翻译流内错误且有半截产物：标记保留（成对；半截按纯文本渲染；TASK-122 真值源 bodyById）',
    store.getState().rawTranslatedIds['201'] === true
    && bodyOf(store.getState(), '201').translatedContent === '<b>半截');

  await bootFixture();
  S.aiTr = { deltas: [], error: null, reject: { message: 'down' }, finish: true, holdIds: [] };
  store.setState({ toasts: [] });
  store.getState().selectArticle('201');
  await nTick(10);
  store.getState().toggleReaderTranslation();
  await nTick(20);
  checkNew('(p3-f3) Reader 翻译 IPC 失败且无半截产物：rawTranslatedIds 清除',
    store.getState().rawTranslatedIds['201'] === undefined
    && store.getState().translateErrors['201'] === 'AI 服务未配置或不可达');

  /* M6（审查：onError 路径不清标记在修后回归网下全绿）：卡片翻译流内错误且
     无半截 → 标记清除（既有 (l2) 只盖 .catch 路径与「有半截保留」侧）。 */
  await bootFixture();
  S.aiTr = { deltas: [], error: '限流', reject: null, finish: true, holdIds: [] };
  store.setState({ toasts: [] });
  store.getState().translateEntry('201');
  await nTick(20);
  checkNew('(p3-l2) 卡片翻译流内错误且无半截产物：rawTranslatedIds 清除（M6 变红）',
    store.getState().rawTranslatedIds['201'] === undefined
    && store.getState().translateErrors['201'] === '限流');

  await bootFixture();
  S.aiTr = { deltas: [], error: null, reject: null, finish: true, holdIds: [201] };
  store.getState().translateEntry('201');
  store.getState().translateEntry('202');
  await nTick(5);
  checkNew('(l) 按 id 隔离：A 仍在生成时 B 已收尾，两个 id 状态互不串台',
    store.getState().translatingIds['201'] === true && store.getState().translatingIds['202'] === undefined);
  const heldTr = S.heldAi.find((h) => h.cmd === 'ai_translate' && h.id === 201);
  heldTr.ch.onmessage?.({ type: 'error', data: '限流' });
  const lTrErr = store.getState();
  checkNew('(l) 翻译流内 error：记录 translateErrors[id] + 清生成态 + toast 带重试',
    lTrErr.translateErrors['201'] === '限流' && lTrErr.translatingIds['201'] === undefined
    && lTrErr.toasts[lTrErr.toasts.length - 1]?.text === '翻译失败：限流'
    && lTrErr.toasts[lTrErr.toasts.length - 1]?.action?.label === '重试');

  await bootFixture();
  store.setState({ toasts: [] });
  S.aiTr = { deltas: [], error: null, reject: { message: 'down' }, finish: true, holdIds: [] };
  store.getState().translateEntry('301', { silent: true });
  await nTick(20);
  checkNew('(l) translateEntry reject →「AI 服务未配置或不可达」，silent 时不弹 toast',
    store.getState().translateErrors['301'] === 'AI 服务未配置或不可达'
    && store.getState().toasts.length === 0);

  store.setState({ dataMode: 'mock' });
  S.invokeCalls.length = 0;
  store.getState().translateEntry('301');
  checkNew('(l) mock 模式不支持 AI：给出提示且不发 IPC',
    S.invokeCalls.filter((c) => c.cmd === 'ai_translate').length === 0
    && store.getState().toasts[store.getState().toasts.length - 1]?.text === '演示模式不支持 AI 服务');

  /* ---------- L1：批量标读索引化后语义不变（只增复杂度优化，行为契约不变） ---------- */
  await bootFixture();
  const l1Ids = store.getState().entries.map((e) => e.id);
  S.invokeCalls.length = 0;
  store.getState().markEntriesReadBulk(l1Ids);
  await nTick(0);   // api.setReadBulk 内部 await getInvoke()，落库是异步 fire-and-forget
  const l1 = store.getState();
  /* AUDIT P3[F4]（TASK-084）：IPC 契约由「6 次 set_read」改为「1 次 set_read_bulk」。
     本断言的**原意是「未读项全部标读、已读项不重复写库」**，该意图完整保留。
     审查 FINDING TASK-084-F2 指出：上一版只钉了载荷**长度**（=== 6），是**弱于**原断言
     的——原来的「set_read 计数 === 6」其实是一个**精确 id 集合检查**（一个 set_read 只可能
     为「即将被写入的未读 id」发出），因此 6 次即证明恰是那 6 个未读 id 被写、且 2 个已读 id
     未被写。现在按原强度把**具体 id 集合**钉死（已读的 102/202 必须不在其中）。 */
  const l1BulkIds = (S.invokeCalls.find((c) => c.cmd === 'set_read_bulk')?.args.ids ?? [])
    .slice().sort((a, b) => a - b).join(',');
  checkNew('(L1) 索引化批量标读：未读项全部标读、已读项不重复写库（1 次 set_read_bulk，载荷恰为 6 个未读 id 101,103,104,105,201,301）',
    l1.entries.every((e) => e.isRead)
    && S.invokeCalls.filter((c) => c.cmd === 'set_read_bulk').length === 1
    && S.invokeCalls.filter((c) => c.cmd === 'set_read').length === 0
    && l1BulkIds === '101,103,104,105,201,301');
  checkNew('(L1) 未读计数仍按源聚合扣减：源10 3→1、源12 2→0、源11 2→1、源20 1→0',
    l1.feedCounts.get('10')?.unread === 1 && l1.feedCounts.get('12')?.unread === 0
    && l1.feedCounts.get('11')?.unread === 1 && l1.feedCounts.get('20')?.unread === 0);
  checkNew('(L1) 「已读保留」快照按被标读的未读项写入（6 条；本就已读的 102/202 不重复记）',
    Object.keys(l1.openedReadIds).length === 6
    && l1.openedReadIds['101'] === true && l1.openedReadIds['105'] === true
    && l1.openedReadIds['102'] === undefined);
  S.invokeCalls.length = 0;
  store.getState().markEntriesReadBulk(['不存在的id']);
  checkNew('(L1) 未知 id 被忽略：不发 IPC、不改状态', S.invokeCalls.length === 0);

  /* ---------- L2：feedCounts 缺项 → 有意的保守设计（本断言即该设计的锚） ---------- */
  await bootFixture();
  const l2Before = selectViewCounts(store.getState()).all;
  const l2Counts = new Map(store.getState().feedCounts);
  l2Counts.delete('12');                               // 模拟后端精确计数缺该源
  store.setState({ feedCounts: l2Counts });
  const l2View = selectViewCounts(store.getState());
  const l2Tree = selectTreeCounts(store.getState());
  checkNew('(L2) feedCounts 缺项：该源不计入「全部」总数（7 → 5），树角标也不建该行（保守设计）',
    l2Before === 7 && l2View.all === 5 && l2Tree.get('all') === 5 && !l2Tree.has('12'));
  const l2Visible = selectVisibleEntries(store.getState());
  checkNew('(L2) 但该源条目仍正常列出（计数缺失不影响内容可见性）',
    l2Visible.filter((e) => e.feedId === '12').length === 2);

  /* ============================================================
     TASK-107（REQ-003）：全部已读与标读计数一致性
     —— 成功以后端实际影响数对账（feed_counts 重取整体替换）、失败回滚不假
     成功、单条标读计数与后端口径一致（含同文副本不重复扣减）、范围外布局/
     范围计数不受影响。
     审计探针（AUDIT-20261005-core-consistency.md）：范围 600 条未读、前端
     加载 1 条，后端成功 600 条后界面仍显示 599 条未读（乐观推算缺口）。
     计数对账用例统一挂 feedCountsImpl = countsFromRows（忠实聚合）；mock 的
     set_read / mark_all_read 已升级为忠实落库（就地翻转 is_read 并返回受影响
     行数），resetStore 对行对象深拷贝防跨用例突变残留。
     ============================================================ */
  {
    /* ---------- t104-markall-count-authoritative：600/1 探针转断言 ---------- */
    await resetStore();
    S.feedCountsImpl = countsFromRows;
    const rows107 = [];
    for (let i = 0; i < 600; i += 1) rows107.push(mkRow({ id: 40000 + i, feed_id: 10, published_at: iso(NOW - i * 1000) }));
    S.backendRows = rows107;
    /* 注意：不能走 bootFixture()——resetStore 会复位 feedCountsImpl/backendRows，
       自定义夹具必须在 resetStore 之后、bootstrapFromBackend 之前就位 */
    await store.getState().bootstrapFromBackend();
    /* 极端分页形态：范围 600 条未读、前端仅加载 1 条（其余 599 条不在册） */
    store.setState((s) => ({ entries: s.entries.slice(0, 1) }));
    S.invokeCalls.length = 0;
    store.getState().markCurrentViewAllRead();
    await nTick(10);
    checkNew('(t104-markall-count-authoritative) 600/1 探针：全部已读成功后未读计数=后端口径 0（修前按已加载推算残留 599）',
      store.getState().feedCounts.get('10')?.unread === 0
      && store.getState().entries.find((a) => a.id === '40000')?.isRead === true
      && S.invokeCalls.filter((c) => c.cmd === 'feed_counts').length === 1);

    /* ---------- t104-markall-failure-rollback：失败不假成功 ---------- */
    await resetStore();
    S.feedCountsImpl = countsFromRows;
    S.backendRows = [
      mkRow({ id: 41001, feed_id: 10, published_at: iso(NOW) }),
      mkRow({ id: 41002, feed_id: 10, published_at: iso(NOW - 60000) }),
      mkRow({ id: 41003, feed_id: 12, published_at: iso(NOW - 120000) }),
      mkRow({ id: 41004, feed_id: 12, published_at: iso(NOW - 180000) }),
      mkRow({ id: 41005, feed_id: 12, published_at: iso(NOW - 240000) }),
    ];
    await store.getState().bootstrapFromBackend();
    S.rejectCmds.add('mark_all_read');
    store.getState().markCurrentViewAllRead();
    await nTick(10);
    const rb107 = store.getState();
    checkNew('(t104-markall-failure-rollback) markAllRead 失败：无「已全部标为已读」假成功提示，失败 toast 可见且带重试',
      !rb107.toasts.some((t) => t.text === '已全部标为已读')
      && rb107.toasts.some((t) => t.text.startsWith('全部已读保存失败') && t.action?.label === '重试'));
    checkNew('(t104-markall-failure-rollback) 失败回滚到位：已读态全部还原、未读计数还原（feed10=2 / feed12=3）',
      rb107.entries.every((a) => !a.isRead)
      && rb107.feedCounts.get('10')?.unread === 2 && rb107.feedCounts.get('12')?.unread === 3);

    /* ---------- t104-single-read-count：单条标读计数与后端口径一致 ---------- */
    await resetStore();
    S.feedCountsImpl = countsFromRows;
    S.backendRows = [
      mkRow({ id: 42001, feed_id: 10, guid: 'dup-1', published_at: iso(NOW) }),
      mkRow({ id: 42002, feed_id: 12, published_at: iso(NOW - 60000) }),
      mkRow({ id: 42003, feed_id: 11, guid: 'dup-1', published_at: iso(NOW - 120000) }),
      mkRow({ id: 42004, feed_id: 11, published_at: iso(NOW - 180000) }),
    ];
    await store.getState().bootstrapFromBackend();
    store.getState().toggleEntryFlag('42001', 'isRead');
    await nTick(10);
    const single107 = store.getState();
    /* 后端口径：set_read 只翻转 42001 自身（同文副本行 42003 不动），按行重算聚合 */
    const agg107 = new Map(countsFromRows().map((c) => [String(c.feed_id), c]));
    checkNew('(t104-single-read-count) 单条标读：前端 feedCounts 与后端按行聚合逐源一致（feed10 恰好 -1，其余不动）',
      single107.feedCounts.get('10')?.unread === agg107.get('10')?.unread && single107.feedCounts.get('10')?.unread === 0
      && single107.feedCounts.get('11')?.unread === agg107.get('11')?.unread && single107.feedCounts.get('11')?.unread === 2
      && single107.feedCounts.get('12')?.unread === agg107.get('12')?.unread && single107.feedCounts.get('12')?.unread === 1);
    checkNew('(t104-single-read-count) 同文副本不重复扣减：主条目标读后，副本行（42003）读态与所属源计数均不变',
      agg107.get('11')?.unread === 2
      && single107.entries.find((a) => a.id === '42003')?.isRead === false);

    /* ---------- t104-scope-isolation：范围外布局/范围计数不受影响 ---------- */
    await resetStore();
    S.feedCountsImpl = countsFromRows;
    S.backendRows = [
      mkRow({ id: 43001, feed_id: 10, published_at: iso(NOW) }),
      mkRow({ id: 43002, feed_id: 10, published_at: iso(NOW - 60000) }),
      mkRow({ id: 43003, feed_id: 11, published_at: iso(NOW - 120000) }),
      mkRow({ id: 43011, feed_id: 12, published_at: iso(NOW - 180000) }),
      mkRow({ id: 43012, feed_id: 12, published_at: iso(NOW - 240000) }),
      mkRow({ id: 43013, feed_id: 12, published_at: iso(NOW - 300000) }),
    ];
    await store.getState().bootstrapFromBackend();
    /* 范围内仅部分条目已加载（分页快照形态）：feed12 共 3 条未读，在册 2 条 */
    store.setState((s) => ({ entries: s.entries.filter((a) => a.id === '43011' || a.id === '43012') }));
    store.setState({ activeFeedFilter: '12' });
    const isoBefore107 = store.getState();
    store.getState().markCurrentViewAllRead();
    await nTick(10);
    const isoAfter107 = store.getState();
    checkNew('(t104-scope-isolation) 单源范围全部已读（3 条未读仅 2 条在册）：该源计数=后端口径 0（修前残留 1）',
      isoAfter107.feedCounts.get('12')?.unread === 0
      && isoAfter107.entries.every((a) => a.feedId === '12' && a.isRead));
    checkNew('(t104-scope-isolation) 范围外不受影响：其他源（feed10=2）与跨布局源（feed11 social=1）计数保持原值',
      isoAfter107.feedCounts.get('10')?.unread === isoBefore107.feedCounts.get('10')?.unread
      && isoAfter107.feedCounts.get('11')?.unread === isoBefore107.feedCounts.get('11')?.unread
      && isoAfter107.feedCounts.get('10')?.unread === 2 && isoAfter107.feedCounts.get('11')?.unread === 1);

    /* ---------- t104-rollback-guard-versioned（TASK-107 R1/F1）：版本化回滚守卫 ----------
       审查探针 C3 场景：全部已读在途失败期间，用户对同一在册条目连点两次 toggle
       停在「已读」（其自身 set_read(true) 已落库）。修前值守卫（当前值==乐观写入值）
       无法区分「用户已接管」与「未被触碰」，迟到回滚把 UI 踩回未读而 DB 是已读，
       计数同步偏差 +1；修后以「条目变更版本未变」为恢复前提，用户接管（每次真实
       翻转都 bump 版本）的条目一律跳过。
       【TASK-118 更新理由】版本键升级为 articleId × field（per-field）后守卫语义
       保持——本场景的接管判定原本只涉及 isRead 写入，为使断言对「per-field 升级」
       有判别力（而非仅锁行为不变），在窗口内对 47002 追加一次**收藏**写入：
       修前（文章级共享版本）收藏 bump 会 void 掉 47002 的读声明、回滚被跳过
       （行卡在乐观已读、计数偏差），修后 isStarred 的 bump 不进 isRead 版本，
       47002 的读回滚照常恢复且收藏保留。原有两断言的条件全部保留（只增不减）。 */
    await resetStore();
    S.feedCountsImpl = countsFromRows;
    S.backendRows = [
      mkRow({ id: 47001, feed_id: 10, published_at: iso(NOW) }),
      mkRow({ id: 47002, feed_id: 10, published_at: iso(NOW - 60000) }),
    ];
    await store.getState().bootstrapFromBackend();
    S.rejectCmds.add('mark_all_read');
    store.getState().markCurrentViewAllRead();            // 乐观翻转 47001/47002 → read
    store.getState().toggleEntryFlag('47001', 'isRead');  // 用户 read→unread（set_read(false) 落库）
    store.getState().toggleEntryFlag('47001', 'isRead');  // 用户 unread→read（最终意图=已读，set_read(true) 落库）
    store.getState().toggleEntryFlag('47002', 'isStarred'); // TASK-118：窗口内收藏写（set_starred(true) 落库；只 bump isStarred 版本）
    await nTick(60);                                       // 全部已读失败回滚落地
    const guard107 = store.getState();
    checkNew('(t104-rollback-guard-versioned) 双 toggle 停在已读 + 全部已读失败：用户最终意图不被回踩（修前值守卫误踩回未读）',
      guard107.entries.find((a) => a.id === '47001')?.isRead === true
      && S.backendRows.find((r) => r.id === 47001)?.is_read === true // R2/F3：wire 格式布尔化（原数字 1）
      && guard107.entries.find((a) => a.id === '47002')?.isRead === false);
    checkNew('(t104-rollback-guard-versioned) 版本化守卫下计数与 DB 真值一致（修前同值误踩会偏差 +1）；窗口内收藏不使 47002 的读回滚失效（per-field：star 只 bump isStarred 版本）且收藏保留（TASK-118 更新）',
      guard107.feedCounts.get('10')?.unread === 1
      && countsFromRows().find((c) => c.feed_id === 10)?.unread === 1
      && guard107.entries.find((a) => a.id === '47002')?.isStarred === true
      && guard107.feedCounts.get('10')?.starred === 1);

    /* ---------- t104-reconcile-retry（TASK-107 R1/F2）：对账重取失败可见化 + 短延迟重试 ----------
       审查探针 F 场景：mark_all_read 落库成功但紧随的 feed_counts 重取失败——
       修前完全静默（计数残留乐观值 599、DB 真值 0，无任何提示）；修后先给一条
       诊断 toast（与「保存失败」文案明确区分），并安排一次 3s 延迟重试。 */
    await resetStore();
    S.feedCountsImpl = countsFromRows;
    const rows107F2 = [];
    for (let i = 0; i < 600; i += 1) rows107F2.push(mkRow({ id: 46000 + i, feed_id: 10, published_at: iso(NOW - i * 1000) }));
    S.backendRows = rows107F2;
    await store.getState().bootstrapFromBackend();
    store.setState((s) => ({ entries: s.entries.slice(0, 1) }));
    S.rejectCmds.add('feed_counts'); // 仅注入对账重取失败（mark_all_read 本身成功落库）
    store.getState().markCurrentViewAllRead();
    await nTick(10);
    const rF2 = store.getState();
    checkNew('(t104-reconcile-retry) 对账重取失败不再静默：诊断 toast 可见且不与「保存失败」混淆（标读本身已成功）',
      rF2.toasts.some((t) => t.text === '全部已读已保存，未读计数刷新失败')
      && !rF2.toasts.some((t) => t.text.startsWith('全部已读保存失败')));
    checkNew('(t104-reconcile-retry) 重取失败时计数停留乐观值（599），等待延迟重试',
      rF2.feedCounts.get('10')?.unread === 599
      && countsFromRows().find((c) => c.feed_id === 10)?.unread === 0);
    S.rejectCmds.delete('feed_counts');
    await nTick(3500); // 等 3s 延迟重试落地
    checkNew('(t104-reconcile-retry) 3s 延迟重试成功：计数自愈为后端真值 0',
      store.getState().feedCounts.get('10')?.unread === 0);

    /* ---------- t104-snapshot-voids-rollback-claim（TASK-107 R2/F3）：快照替换使在途乐观声明失效 ----------
       R1 审查变异发现：mergeSnapshotEntries 对存活 id 的 bumpEntryVersion 无断言覆盖（删掉全绿）。
       本场景给 bump 一个**判别性**用例：全部已读在途失败窗口内，外部 DB 写入（同步拉取把远端
       已读态直接落库——不经前端写入路径、不 bump 前端版本）把行置为已读，随后快照替换带来
       行级真值（行 read=true、计数重取 0）。mark_all_read 此刻才失败：条目当前值==乐观写入值
       （值守卫放行），唯一的守卫是 merge bump（版本已前进）——迟到回滚必须跳过，否则会把
       陈旧未读踩回 UI 并把刚重取的真值计数虚增回去。 */
    await resetStore();
    S.feedCountsImpl = countsFromRows;
    S.backendRows = [
      mkRow({ id: 48001, feed_id: 10, published_at: iso(NOW) }),
      mkRow({ id: 48002, feed_id: 10, published_at: iso(NOW - 60000) }),
    ];
    await store.getState().bootstrapFromBackend();
    const realInvoke107v = globalThis.__INVOKE__;
    let rejectMarkAll107v = null;
    // R1：__INVOKE__ 覆盖必须 finally 复原（成功/异常都保全原值）
    const restoreInvoke107v = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
      if (cmd === 'mark_all_read') return new Promise((_resolve, rej) => { rejectMarkAll107v = rej; });
      return realInvoke107v(cmd, args);
    });
    try {
      store.getState().markCurrentViewAllRead(); // 乐观翻转 48001/48002 → read（计数 2→0，版本快照 v1）
      await nTick(0);
      /* 外部写入者（同步拉取语义）：绕过前端直接落库，行 is_read=1——不触发 flipEntryFlag/markEntriesRead */
      S.backendRows.forEach((r) => { r.is_read = true; }); // 外部落库为 wire 布尔
      await store.getState().reloadFromBackend(); // 快照替换：行 read=true、计数重取 0、merge bump → v2
      const voided107 = store.getState();
      checkNew('(t104-snapshot-voids-rollback-claim) 场景成立：快照替换带来后端真值（行 read=true、计数重取 0）',
        voided107.entries.every((a) => a.isRead) && voided107.feedCounts.get('10')?.unread === 0);
      rejectMarkAll107v({ message: '注入失败:mark_all_read' });
      await nTick(10);
    } finally {
      restoreInvoke107v();
    }
    const afterVoid107 = store.getState();
    checkNew('(t104-snapshot-voids-rollback-claim) 迟到回滚不踩快照真值：行保持 read、计数保持 0（无 bump 时会被恢复为未读并虚增回 2）',
      afterVoid107.entries.every((a) => a.isRead)
      && afterVoid107.feedCounts.get('10')?.unread === 0);
    checkNew('(t104-snapshot-voids-rollback-claim) 回滚跳过不等于吞错：失败 toast 仍可见且带重试',
      afterVoid107.toasts.some((t) => t.text.startsWith('全部已读保存失败') && t.action?.label === '重试'));

    /* ---------- t104-snapshot-fresh-claim-rollback（TASK-107 R2/F3）：快照后新声明的回滚仍正常 ----------
       边界：bump 只使「快照替换**之前**建立的乐观声明」失效，不得永久瘫痪回滚机制。
       两段验证——①快照替换（行仍未读：mark_all_read 未落库）+ 失败：被快照覆盖的条目不被
       陈旧回滚踩到（读态/计数保持快照后的后端真值；此形态值守卫与版本守卫双保险，锁定契约）；
       ②快照替换**之后**新发起的全部已读（版本快照取自 bump 后的现值）失败时，回滚照常
       恢复读态并回补计数——机制本身仍然存活。 */
    await resetStore();
    S.feedCountsImpl = countsFromRows;
    S.backendRows = [
      mkRow({ id: 49001, feed_id: 10, published_at: iso(NOW) }),
      mkRow({ id: 49002, feed_id: 10, published_at: iso(NOW - 60000) }),
    ];
    await store.getState().bootstrapFromBackend();
    const realInvoke107f = globalThis.__INVOKE__;
    let rejectMarkAll107f = null;
    // R1：__INVOKE__ 覆盖必须 finally 复原
    const restoreInvoke107f = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
      if (cmd === 'mark_all_read') return new Promise((_resolve, rej) => { rejectMarkAll107f = rej; });
      return realInvoke107f(cmd, args);
    });
    try {
      store.getState().markCurrentViewAllRead(); // 声明 #1：乐观翻转 → read（计数 2→0）
      await nTick(0);
      await store.getState().reloadFromBackend(); // 快照替换（行仍未读：mark_all_read 未落库）→ 计数重取 2、bump
      rejectMarkAll107f({ message: '注入失败:mark_all_read' });
      await nTick(10);
      const staleClaim107 = store.getState();
      checkNew('(t104-snapshot-fresh-claim-rollback) 陈旧声明失效：快照后行保持未读、计数保持重取值 2（不被陈旧回滚踩动）',
        staleClaim107.entries.every((a) => !a.isRead)
        && staleClaim107.feedCounts.get('10')?.unread === 2);
      store.getState().markCurrentViewAllRead(); // 声明 #2（快照后新建）：乐观翻转 → read（计数 2→0）
      await nTick(0);
      rejectMarkAll107f({ message: '注入失败:mark_all_read' });
      await nTick(10);
    } finally {
      restoreInvoke107f();
    }
    const freshClaim107 = store.getState();
    checkNew('(t104-snapshot-fresh-claim-rollback) 快照后新声明的回滚仍正常：两行恢复未读、计数回补到 2（机制未被 bump 瘫痪）',
      freshClaim107.entries.every((a) => !a.isRead)
      && freshClaim107.feedCounts.get('10')?.unread === 2
      && freshClaim107.toasts.some((t) => t.text.startsWith('全部已读保存失败')));
  }

  /* ============================================================
     TASK-118（2026-10-07，审计 P1-2）：跨字段操作版本与回滚统一——
     收藏不再使读状态回滚失效。

     审计探针本体（tmp/audit-20261007/probes.mjs P2）：entryMutationVersion
     修前是文章级共享版本——全部已读在途时用户收藏该文，收藏 bump 同一版本，
     读状态回滚被当成「已被接管」而跳过（审计实测：应 isRead=false/unread=1，
     实际 isRead=true/isStarred=true/unread=0）。修法：版本键改 articleId ×
     field（isRead/isStarred 各自单调）；markCurrentViewAllRead 回滚只查
     isRead 字段版本；回滚统一——单条 toggle / 批量标读 / 打开即标读三条
     路径失败都走同一回滚助手 rollbackEntryClaims（审计相邻缺口：「所有
     乐观写入都有统一版本回滚」此前不成立——bulk 与打开即标读失败只提示）。

     门禁纪律（DEC-gate-adjust-20261007）：审计探针场景转真实行为回归。
     本组全部走真实 store 动作序列；假后端复用既有 rejectCmds（按命令名
     注入失败）与 held-promise（手动 reject 驱动在途窗口）机制，无需扩展
     注入设施（set_read_bulk / set_read 的拒绝注入由 rejectCmds 直接覆盖）。
     ============================================================ */
  {
    const it118 = await import('../../dist-test/store/internals.js');

    /* ---------- t118-probe-cross-field：审计探针本体（全部已读在途+收藏+失败） ---------- */
    await resetStore();
    S.feedCountsImpl = countsFromRows;
    S.backendRows = [mkRow({ id: 52001, feed_id: 10, published_at: iso(NOW) })];
    await store.getState().bootstrapFromBackend();
    const realInvoke118 = globalThis.__INVOKE__;
    let rejectMarkAll118 = null;
    // R1：__INVOKE__ 覆盖必须 finally 复原
    const restoreInvoke118 = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
      if (cmd === 'mark_all_read') return new Promise((_res, rej) => { rejectMarkAll118 = rej; });
      return realInvoke118(cmd, args);
    });
    try {
      store.getState().markCurrentViewAllRead();              // 全部已读在途（乐观：read、unread 0，isRead 版本快照 v1）
      await nTick(0);
      store.getState().toggleEntryFlag('52001', 'isStarred'); // 用户收藏（set_starred(true) 落库；只 bump isStarred 版本）
      await nTick(0);
      rejectMarkAll118({ message: '注入失败:mark_all_read' }); // 全部已读此刻才失败
      await nTick(10);
    } finally {
      restoreInvoke118();
    }
    const probe118 = store.getState();
    checkNew('(t118-probe-cross-field) 探针本体：全部已读在途+收藏+失败 → 读状态恢复未读（修前收藏 bump 共享版本使读回滚被跳过：isRead 停留 true）',
      probe118.entries.find((a) => a.id === '52001')?.isRead === false
      && probe118.feedCounts.get('10')?.unread === 1);
    checkNew('(t118-probe-cross-field) 探针本体：收藏保留（isStarred=true、starred=1，与后端按行聚合一致——读失败不牵连收藏）',
      probe118.entries.find((a) => a.id === '52001')?.isStarred === true
      && probe118.feedCounts.get('10')?.starred === 1
      && countsFromRows().find((c) => c.feed_id === 10)?.starred === 1);

    /* ---------- t118-field-version：跨字段版本隔离真值表（三个 bump 点） ---------- */
    await resetStore();
    S.backendRows = [
      mkRow({ id: 56001, feed_id: 10, published_at: iso(NOW) }),
      mkRow({ id: 56002, feed_id: 10, published_at: iso(NOW - 60000) }),
    ];
    await store.getState().bootstrapFromBackend();
    const v118 = (id, field) => it118.getEntryVersion(id, field);
    /* 基线说明：bootstrapFromBackend 落地的后端快照本身就是一次真实写入
       （mergeSnapshotEntries fromBackend=true 两字段 bump）——启动后每行两字段
       版本 = 1；版本 0 只属于从未被任何写入触达的 (id, field)。 */
    checkNew('(t118-field-version) 真值表·基线：启动快照计入版本（每行两字段=1），未触达的 (id, field) 为 0',
      v118('56001', 'isRead') === 1 && v118('56001', 'isStarred') === 1
      && v118('56002', 'isRead') === 1 && v118('56002', 'isStarred') === 1
      && v118('99999', 'isRead') === 0 && v118('99999', 'isStarred') === 0);
    store.getState().toggleEntryFlag('56001', 'isStarred'); // 收藏：flipEntryFlag 按旗标 bump isStarred
    checkNew('(t118-field-version) 真值表·收藏：isStarred 2、isRead 保持 1（跨字段隔离核心——修前共享版本两者同 bump）',
      v118('56001', 'isStarred') === 2 && v118('56001', 'isRead') === 1);
    store.getState().toggleEntryFlag('56002', 'isRead');    // 单条标读：flipEntryFlag bump isRead
    it118.markEntriesRead(new Set(['56001']));              // 批量标读（本地乐观）：只 bump isRead
    checkNew('(t118-field-version) 真值表·标读：isRead bump、同条目 isStarred 与另一条目 isStarred 均不受牵连',
      v118('56001', 'isRead') === 2 && v118('56001', 'isStarred') === 2
      && v118('56002', 'isRead') === 2 && v118('56002', 'isStarred') === 1);
    await store.getState().reloadFromBackend();             // 后端快照替换（fromBackend=true）
    checkNew('(t118-field-version) 真值表·后端快照：两字段都 bump（快照整体替换携带行级真值，两字段在途声明一并失效）',
      v118('56001', 'isRead') === 3 && v118('56001', 'isStarred') === 3
      && v118('56002', 'isRead') === 3 && v118('56002', 'isStarred') === 2);

    /* ---------- t118-symmetric-cross-field：对称方向（收藏在途+标读接管+收藏失败） ---------- */
    await resetStore();
    S.feedCountsImpl = countsFromRows;
    S.backendRows = [mkRow({ id: 57001, feed_id: 10, published_at: iso(NOW) })];
    await store.getState().bootstrapFromBackend();
    const realInvoke118s = globalThis.__INVOKE__;
    let rejectSetStarred118s = null;
    // R1：__INVOKE__ 覆盖必须 finally 复原
    const restoreInvoke118s = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
      if (cmd === 'set_starred') return new Promise((_res, rej) => { rejectSetStarred118s = rej; });
      return realInvoke118s(cmd, args);
    });
    try {
      store.getState().toggleEntryFlag('57001', 'isStarred'); // 收藏在途（乐观 starred、计数 +1）
      await nTick(0);
      store.getState().toggleEntryFlag('57001', 'isRead');    // 用户标读（set_read(true) 落库成功；bump isRead 版本）
      await nTick(0);
      rejectSetStarred118s({ message: '注入失败:set_starred' }); // 收藏此刻才失败
      await nTick(10);
    } finally {
      restoreInvoke118s();
    }
    const sym118 = store.getState();
    checkNew('(t118-symmetric-cross-field) 对称方向：收藏失败回滚仍恢复（isStarred=false、starred=0）——标读 bump 的是 isRead 版本，不 void 收藏声明（修前共享版本下收藏停留乐观 true）',
      sym118.entries.find((a) => a.id === '57001')?.isStarred === false
      && sym118.feedCounts.get('10')?.starred === 0);
    checkNew('(t118-symmetric-cross-field) 对称方向：窗口内的标读保留（isRead=true、unread=0，与 DB 一致）',
      sym118.entries.find((a) => a.id === '57001')?.isRead === true
      && sym118.feedCounts.get('10')?.unread === 0
      && countsFromRows().find((c) => c.feed_id === 10)?.unread === 0);

    /* ---------- t118-bulk-rollback：set_read_bulk 失败回滚（统一助手，批量路径） ---------- */
    await resetStore();
    S.feedCountsImpl = countsFromRows;
    S.backendRows = [
      mkRow({ id: 53001, feed_id: 10, published_at: iso(NOW) }),
      mkRow({ id: 53002, feed_id: 10, published_at: iso(NOW - 60000) }),
      mkRow({ id: 53003, feed_id: 10, published_at: iso(NOW - 120000) }),
    ];
    await store.getState().bootstrapFromBackend();
    S.rejectCmds.add('set_read_bulk');
    store.getState().markEntriesReadBulk(['53001', '53002', '53003']); // 乐观翻转 3 行 → read、unread 0
    store.getState().toggleEntryFlag('53001', 'isRead');  // 窗口内用户双击接管 53001：read→unread（set_read(false) 落库）
    store.getState().toggleEntryFlag('53001', 'isRead');  // unread→read（最终意图=已读，set_read(true) 落库）
    await nTick(10);                                       // bulk 失败回滚落地
    const bulk118 = store.getState();
    checkNew('(t118-bulk-rollback) set_read_bulk 失败：未被接管的行（53002/53003）恢复未读、unread 逐 feed 回补、失败 toast 可见（修前只提示不回滚）',
      bulk118.entries.find((a) => a.id === '53002')?.isRead === false
      && bulk118.entries.find((a) => a.id === '53003')?.isRead === false
      && bulk118.feedCounts.get('10')?.unread === 2
      && bulk118.toasts.some((t) => t.text === '批量标读失败'));
    checkNew('(t118-bulk-rollback) 版本守卫：窗口内双击停在读的 53001 不被回踩（isRead 版本已前进），UI 与 DB 真值一致（修前无回滚/值守卫均无法同时满足）',
      bulk118.entries.find((a) => a.id === '53001')?.isRead === true
      && S.backendRows.find((r) => r.id === 53001)?.is_read === true
      && bulk118.feedCounts.get('10')?.unread === countsFromRows().find((c) => c.feed_id === 10)?.unread);

    /* ---------- t118-open-read-rollback：打开即标读失败回滚（统一助手，selectArticle 路径） ---------- */
    await resetStore();
    S.feedCountsImpl = countsFromRows;
    S.backendRows = [mkRow({ id: 54001, feed_id: 10, published_at: iso(NOW) })];
    await store.getState().bootstrapFromBackend();
    S.rejectCmds.add('set_read');
    store.getState().selectArticle('54001'); // markReadOnOpen 默认 true → 乐观置 read + set_read 失败
    await nTick(10);
    const open118 = store.getState();
    checkNew('(t118-open-read-rollback) 打开即标读失败：读态恢复未读、unread 回补（修前乐观停留已读只提示）+ 失败 toast 可见',
      open118.entries.find((a) => a.id === '54001')?.isRead === false
      && open118.feedCounts.get('10')?.unread === 1
      && open118.toasts.some((t) => t.text.startsWith('标读失败：')));
    checkNew('(t118-open-read-rollback) 边界裁定：回滚读态但不清阅读上下文——activeArticleId 保留（用户还在读）、「打开过」保留标记在（未读筛选下原地变灰而非消失）',
      open118.activeArticleId === '54001'
      && open118.openedReadIds['54001'] === true);

    /* ---------- t118-markall-twice：连续两次全部已读失败（版本快照可复用性） ---------- */
    await resetStore();
    S.feedCountsImpl = countsFromRows;
    S.backendRows = [
      mkRow({ id: 58001, feed_id: 10, published_at: iso(NOW) }),
      mkRow({ id: 58002, feed_id: 10, published_at: iso(NOW - 60000) }),
    ];
    await store.getState().bootstrapFromBackend();
    S.rejectCmds.add('mark_all_read');
    store.getState().markCurrentViewAllRead();
    await nTick(10); // 第一轮失败回滚（两行恢复未读、版本停在 v1）
    checkNew('(t118-markall-twice) 第一轮全部已读失败：回滚到位（两行未读、unread=2）',
      store.getState().entries.every((a) => !a.isRead)
      && store.getState().feedCounts.get('10')?.unread === 2);
    store.getState().markCurrentViewAllRead();
    await nTick(10); // 第二轮失败回滚：乐观写入重新翻转（bump → v2），快照取自首轮回滚后的现值
    checkNew('(t118-markall-twice) 第二轮全部已读失败：新快照（v2）下回滚照常恢复——机制不被首轮 bump/回滚瘫痪、计数不重复回补（两行未读、unread=2、无假成功提示）',
      store.getState().entries.every((a) => !a.isRead)
      && store.getState().feedCounts.get('10')?.unread === 2
      && S.backendRows.every((r) => !r.is_read)
      && !store.getState().toasts.some((t) => t.text === '已全部标为已读'));

    await resetStore(); // 夹具复位：不把 rejectCmds / 大夹具残留带给后续块
  }
}
