// tools/frontend-tests/subscription-writeback.mjs
// 领域模块：OPT-009 B3 订阅写回可见性——能力矩阵投影、回执摘要与状态动作。
//
// 目的（本域锁定的行为契约）：
// - `subscriptionWriteSummary`（src/lib/syncPill.ts 纯函数）真值表：
//   无待办 → 空串（X3 口径：不新增噪音行）；pending/failed → 等待 N 项 /
//   其中 M 项失败（最近错误 ≤60 码点截断）；local_only → 必须如实点名
//   「仅本地生效」并携带协议事实（Fever ≠ 已同步）；组合输入两段并存。
// - `subscriptionActionLine`：能力矩阵逐动作投影（Supported→已支持 /
//   LocalOnly→仅本地 / Unsupported→不支持），Fever 必须全行可读「仅本地」。
// - `deleteRemoteLine`：删除确认的远端语义句**唯一取自** subscription_capabilities
//   （同源，不另写一套）；caps 未加载返回 null（调用方省略，不冒充）。
// - store 动作：`applySubscriptionOutboxChanged` 只写回执四字段（不触碰
//   syncConnected/队列统计）；`refreshSubscriptionOutboxStats` 失败**保持原值**
//   （不得写 0 冒充「无待办」）；`loadSubscriptionCapabilities` 失败保持 null。
//
// 取证方式：真实 store + 纯函数直驱 + 受控 IPC（__INVOKE__ 覆盖，finally 恢复）。
export const id = 'subscription-writeback';

export async function run(ctx) {
  const { S, store, checkNew, nTick, bootFixture, overrideGlobal } = ctx;
  /* 经 ctx 调用（不要解构成 use* 前缀的局部名：oxlint react-hooks 规则会把
     它当 Hook 调用误报）。 */
  await ctx.useMainBackend();
  await bootFixture();
  await nTick(20); /* 隔离 bootstrap 顺带的 void 承诺（迟到落地会覆写 store） */

  const { subscriptionWriteSummary, subscriptionActionLine, deleteRemoteLine } =
    await import('../../src/lib/syncPill.ts');

  /* ============================================================
     A：摘要真值表（无待办 / pending / failed / local_only / Fever 五输入）
     ============================================================ */
  const st = (o) => ({ pending: 0, failed: 0, local_only: 0, last_error: null, ...o });

  checkNew('(opt009b3-sum-1) 无待办 → 空串（不新增噪音行）',
    subscriptionWriteSummary(st({}), 'greader') === ''
    && subscriptionWriteSummary(st({}), 'fever') === ''
    && subscriptionWriteSummary(st({}), null) === '');

  checkNew('(opt009b3-sum-2) pending → 「订阅写回：等待 N 项」',
    subscriptionWriteSummary(st({ pending: 2 }), 'greader') === '订阅写回：等待 2 项');

  checkNew('(opt009b3-sum-3) failed（pending 子集）→ 部分失败 + 最近错误；错误按码点截断 60 字符（不劈代理对）',
    subscriptionWriteSummary(st({ pending: 2, failed: 1, last_error: '订阅编辑 https://x 推送失败: HTTP 500' }), 'greader')
      === '订阅写回：等待 2 项，其中 1 项失败（订阅编辑 https://x 推送失败: HTTP 500）'
    && subscriptionWriteSummary(st({ pending: 1, failed: 1, last_error: '错'.repeat(80) }), 'greader')
      === `订阅写回：等待 1 项，其中 1 项失败（${'错'.repeat(60)}…）`
    && subscriptionWriteSummary(st({ pending: 1, failed: 1 }), 'greader')
      === '订阅写回：等待 1 项，其中 1 项失败');

  checkNew('(opt009b3-sum-4) local_only → 必须如实「仅本地生效」且点名协议（Fever ≠ 已同步；不得写成成功也不得隐藏）',
    subscriptionWriteSummary(st({ local_only: 3 }), 'fever')
      === '订阅写回：3 项仅本地生效（Fever 协议不支持远端写回）'
    && subscriptionWriteSummary(st({ local_only: 1 }), null)
      === '订阅写回：1 项仅本地生效（当前协议不支持远端写回）');

  checkNew('(opt009b3-sum-5) 组合输入（pending + local_only）两段并存，不互相吞并',
    subscriptionWriteSummary(st({ pending: 1, failed: 1, local_only: 2, last_error: '网络失败' }), 'fever')
      === '订阅写回：等待 1 项，其中 1 项失败（网络失败）；2 项仅本地生效（Fever 协议不支持远端写回）');

  /* ============================================================
     B：能力矩阵逐动作投影（Fever 行必须全部可见「仅本地」）
     ============================================================ */
  const cap = (support, detail = 'x') => ({ support, detail });
  const capsOf = (p, s) => ({
    connected: p !== null,
    protocol: p,
    overview: '概述',
    subscribe: cap(s.sub ?? s.all ?? 'Supported'),
    edit_title: cap(s.edit ?? s.all ?? 'Supported'),
    move_folder: cap(s.move ?? s.all ?? 'Supported'),
    unsubscribe: cap(s.unsub ?? s.all ?? 'Supported'),
    rename_folder: cap(s.rename ?? s.all ?? 'Supported'),
    delete_folder: cap(s.del ?? s.all ?? 'Supported'),
    empty_folder_remote: cap(s.empty ?? 'Unsupported'),
    limitations: [],
  });

  const gLine = subscriptionActionLine(capsOf('greader', { all: 'Supported' }));
  const fLine = subscriptionActionLine(capsOf('fever', { all: 'LocalOnly' }));
  checkNew('(opt009b3-cap-1) 投影标签逐动作正确（GReader 全「已支持」+ 空分类「不支持」；Fever 全「仅本地」——编辑/新增/退订不出现「已支持」错觉）',
    gLine === '新增 已支持 · 编辑 已支持 · 移动 已支持 · 退订 已支持 · 分类改名 已支持 · 分类删除 已支持 · 空分类 不支持'
    && fLine === '新增 仅本地 · 编辑 仅本地 · 移动 仅本地 · 退订 仅本地 · 分类改名 仅本地 · 分类删除 仅本地 · 空分类 不支持');

  /* ============================================================
     C：删除确认远端语义句（与能力矩阵同源；未加载不冒充）
     ============================================================ */
  const gCaps = capsOf('greader', { all: 'Supported' });
  gCaps.unsubscribe = cap('Supported', 'G-DEL');
  gCaps.delete_folder = cap('Supported', 'G-FOL');
  gCaps.empty_folder_remote = cap('Unsupported', 'G-EMPTY');
  const fCaps = capsOf('fever', { all: 'LocalOnly' });
  fCaps.unsubscribe = cap('LocalOnly', 'F-DEL');
  fCaps.delete_folder = cap('LocalOnly', 'F-FOL');
  fCaps.empty_folder_remote = cap('Unsupported', 'F-EMPTY');

  checkNew('(opt009b3-del-1) 删除源：直接取 unsubscribe.detail（单一事实源）；分类：有成员取 delete_folder.detail、空分类取 empty_folder_remote.detail',
    deleteRemoteLine(gCaps, 'feed', 0) === 'G-DEL'
    && deleteRemoteLine(gCaps, 'folder', 2) === 'G-FOL'
    && deleteRemoteLine(gCaps, 'folder', 0) === 'G-EMPTY'
    && deleteRemoteLine(fCaps, 'feed', 0) === 'F-DEL'
    && deleteRemoteLine(fCaps, 'folder', 1) === 'F-FOL');

  checkNew('(opt009b3-del-2) caps 未加载 → null（调用方省略该句；不编造远端语义）',
    deleteRemoteLine(null, 'feed', 0) === null && deleteRemoteLine(null, 'folder', 2) === null);

  /* ============================================================
     D：store 动作——回执四字段单一状态源 + 失败不冒充 + 能力加载
     ============================================================ */
  const prevInvoke = globalThis.__INVOKE__;
  const be = { stats: null, failStats: false, caps: null, failCaps: false };
  const restoreInvoke = overrideGlobal(globalThis, '__INVOKE__', (cmd, args) => {
    S.invokeCalls.push({ cmd, args });
    switch (cmd) {
      case 'subscription_outbox_stats':
        if (be.failStats) return Promise.reject({ message: '注入：回执读取失败' });
        return Promise.resolve(be.stats);
      case 'subscription_capabilities':
        if (be.failCaps) return Promise.reject({ message: '注入：能力读取失败' });
        return Promise.resolve(be.caps);
      default:
        return prevInvoke(cmd, args);
    }
  });

  try {
    /* 前置：清空回执与能力状态，防止跨用例残留 */
    store.setState({ subOutboxStats: null, subscriptionCapabilities: null });

    /* D-1：事件落地只写回执四字段，不触碰队列统计/连接态 */
    store.setState({ syncConnected: true, syncWaiting: 7, syncFailed: 0, syncQueueLastError: null });
    store.getState().applySubscriptionOutboxChanged({
      pending: 2, failed: 1, local_only: 3, last_error: '订阅推送失败: HTTP 500',
    });
    const s1 = store.getState();
    checkNew('(opt009b3-store-1) applySubscriptionOutboxChanged：回执四字段按事件载荷写入，且不触碰 syncConnected / syncWaiting / syncFailed / syncQueueLastError（越权防护）',
      s1.subOutboxStats?.pending === 2 && s1.subOutboxStats?.failed === 1
      && s1.subOutboxStats?.local_only === 3 && s1.subOutboxStats?.last_error === '订阅推送失败: HTTP 500'
      && s1.syncConnected === true && s1.syncWaiting === 7 && s1.syncFailed === 0 && s1.syncQueueLastError === null);
    /* 摘要随事件渲染 */
    checkNew('(opt009b3-store-2) 事件载荷 → 摘要行（Fever：等待 + 仅本地两段并存）',
      subscriptionWriteSummary(s1.subOutboxStats, 'fever')
        === '订阅写回：等待 2 项，其中 1 项失败（订阅推送失败: HTTP 500）；3 项仅本地生效（Fever 协议不支持远端写回）');

    /* D-2：挂载/手动刷新走真实 IPC 读回（成功路径） */
    be.stats = { pending: 5, failed: 0, local_only: 0, last_error: null };
    await store.getState().refreshSubscriptionOutboxStats();
    checkNew('(opt009b3-store-3) refreshSubscriptionOutboxStats：真实 IPC 读回写入 store（挂载/手动刷新与事件同一状态源）',
      S.invokeCalls.filter((c) => c.cmd === 'subscription_outbox_stats').length === 1
      && store.getState().subOutboxStats?.pending === 5);

    /* D-3：读取失败必须保持原值——不得写 0 冒充「无待办」 */
    be.failStats = true;
    await store.getState().refreshSubscriptionOutboxStats();
    checkNew('(opt009b3-store-4) refreshSubscriptionOutboxStats 失败：保持原值（读失败不得冒充「无待办」）',
      store.getState().subOutboxStats?.pending === 5);

    /* D-4：能力矩阵加载（成功/失败保持） */
    be.caps = gCaps;
    await store.getState().loadSubscriptionCapabilities();
    checkNew('(opt009b3-store-5) loadSubscriptionCapabilities：成功写入能力矩阵；失败保持原值（不伪造未连接/不伪造能力句）',
      store.getState().subscriptionCapabilities?.protocol === 'greader'
      && store.getState().subscriptionCapabilities?.unsubscribe?.detail === 'G-DEL');
    be.failCaps = true;
    await store.getState().loadSubscriptionCapabilities();
    checkNew('(opt009b3-store-6) 能力读取失败：保持上次成功值（不折成 null 也不折成未连接）',
      store.getState().subscriptionCapabilities?.protocol === 'greader');
  } finally {
    restoreInvoke();
    await ctx.resetStore();
  }
}
