// tools/frontend-tests/error-center.mjs
// 领域模块：错误中心与重试入口持久可达 —— OPT-017E。
//
// 保险丝（先红后绿）：本文件在**旧实现**上运行时，新 API/字段尚不存在——
// 字段读取一律 `?? 兜底`、新动作调用一律 `?.()`（可选调用），使红证据是
// 「逐条 ❌ 断言」而不是中途 TypeError（其余域照常执行、日志可读）。
// 实现落地后这些断言全部有判别力（变异 M1–M5 逐项钉住，见 RESULT §3）。
//
// 证据分层（与其他域同口径）：
// - 纯函数真值表：upsertErrorEntry（同 key 合并替换 / 超上限逐出最旧 / 不改入参）、
//   pruneErrorEntries（TTL 生命周期上限）；
// - store 层真实动作序列：连续 10 条带重试错误全部可追溯、超上限有界、
//   同 key（显式实体 key 与缺省文案键）合并且保留最新动作、不同 key 互不覆盖、
//   **同文案不同实体不合并（R1）**、重试派发清除条目、失败（重报/同步抛错）保留
//   入口、普通 toast 不进入错误中心且时长口径不回归、toast 退场后条目仍在；
// - 源级守卫：App/Sidebar 接线 + 全部重试调用点带显式实体 key（迁移完整性）。
// 组件侧真实挂载（入口可见/面板可关闭/点击重试与忽略真实生效）见
// tools/component-tests/error-center.test.mjs；未测边界见 RESULT.md。
export const id = 'error-center';

export async function run(ctx) {
  const { store, checkNew, resetStore } = ctx;
  await ctx.useMainBackend();
  await resetStore();

  const uiMod = await import('../../dist-test/store/slices/ui.js');
  const { ERROR_CENTER_MAX, ERROR_CENTER_TTL_MS, upsertErrorEntry, pruneErrorEntries } = uiMod;

  const errorsOf = () => store.getState().errors ?? [];
  const mkEntry = (key, at, count = 1) => ({
    id: at, key, text: key, count, at, action: { label: '重试', run: () => {} },
  });

  /* ============================================================
     (e0) 常量与纯函数导出（错误中心的存在性底座）
     ============================================================ */
  checkNew('(e0) 错误中心导出存在：上限 20 条、生命周期 TTL 24h、纯函数 upsert/prune',
    ERROR_CENTER_MAX === 20 && ERROR_CENTER_TTL_MS === 24 * 3600 * 1000
    && typeof upsertErrorEntry === 'function' && typeof pruneErrorEntries === 'function');

  /* ============================================================
     (e1) 反例 1：连续 10 条带重试错误——全部可追溯 + 条数受上限约束
     ============================================================ */
  store.setState({ toasts: [], errors: [] });
  const runs10 = [];
  for (let i = 0; i < 10; i += 1) {
    store.getState().showToast(`后台刷新：源${i} 抓取失败`, { label: '重试', run: () => runs10.push(i) });
  }
  const s10 = store.getState();
  const center10 = errorsOf();
  checkNew('(e1) 唯一入口不因 toast 上限被挤掉：连续 10 条带重试错误逐条在错误中心可追溯、逐条可重试（写红时旧实现无错误中心，前 6 条入口已不可触达）',
    center10.length === 10
    && center10.every((e) => typeof e.action?.run === 'function')
    && center10[0].text.includes('源0')
    && center10[9].text.includes('源9'));
  checkNew('(e1) 错误中心按发生顺序排列（最早 → 最新），text/key/count/at 字段完备',
    center10.map((e) => e.text).join(',') === Array.from({ length: 10 }, (_, i) => `后台刷新：源${i} 抓取失败`).join(',')
    && center10.every((e) => e.key === e.text && e.count === 1 && typeof e.at === 'number'));
  checkNew('(e1) toast 瞬时层上限仍为 4 条且保留最新（既有 `(i)` 用例口径不回退——错误中心不改变 toast 层）',
    s10.toasts.length === 4
    && s10.toasts.map((t) => t.text).join(',')
      === '后台刷新：源6 抓取失败,后台刷新：源7 抓取失败,后台刷新：源8 抓取失败,后台刷新：源9 抓取失败');
  checkNew('(e1) 修前形态可复现：同场景旧 toast 队列只剩最后 4 条——前 6 条重试动作在 UI 上已不可触达（本卡要消掉的缺陷）',
    s10.toasts.filter((t) => t.action).length === 4
    && !s10.toasts.some((t) => t.text.includes('源5')));
  checkNew('(e1) 最新条目的动作按下即触发对应负载（追溯 = 可 actionable，不是只存一条死文案）',
    (() => {
      const last = center10[9];
      if (!last?.action?.run) return false;
      last.action.run();
      return runs10.length === 1 && runs10[0] === 9;
    })());

  /* ---- 上限：只有超过 ERROR_CENTER_MAX 才逐出最旧 ---- */
  store.setState({ toasts: [], errors: [] });
  for (let i = 0; i < ERROR_CENTER_MAX + 5; i += 1) {
    store.getState().showToast(`批量错误 ${i}`, { label: '重试', run: () => {} });
  }
  const capErrors = errorsOf();
  checkNew(`(e1) 有界内存：${ERROR_CENTER_MAX + 5} 条连发后错误中心恒为 ${ERROR_CENTER_MAX} 条、保留最新（最旧 5 条被逐出），队列长度与输入总量无关`,
    capErrors.length === ERROR_CENTER_MAX
    && capErrors[0].text === '批量错误 5'
    && capErrors[ERROR_CENTER_MAX - 1].text === `批量错误 ${ERROR_CENTER_MAX + 4}`);

  /* ============================================================
     (e2) 反例 2：同 key 合并且保留最新动作；不同 key 不互相覆盖
     ============================================================ */
  store.setState({ toasts: [], errors: [] });
  const runs2 = [];
  const runOld = () => runs2.push('old');
  const runNew = () => runs2.push('new');
  const runOther = () => runs2.push('other');
  store.getState().showToast('刷新失败：超时', { label: '重试', run: runOld });
  store.getState().showToast('同步失败：网络不可达', { label: '重试', run: runOther });
  store.getState().showToast('刷新失败：超时', { label: '重试', run: runNew }); // 同一故障再次失败
  const eTimeout = errorsOf().find((e) => e.key === '刷新失败：超时');
  const eOther = errorsOf().find((e) => e.key === '同步失败：网络不可达');
  checkNew('(e2) 同 key 合并为一条、count=2，且保留最新动作（旧动作不残留）',
    errorsOf().length === 2 && eTimeout?.count === 2 && eTimeout?.action?.run === runNew);
  checkNew('(e2) 合并后的条目落在最新位（列表顺序反映最近失败序，面板按此呈现）',
    errorsOf()[1]?.key === '刷新失败：超时' && errorsOf()[0]?.key === '同步失败：网络不可达');
  checkNew('(e2) 不同 key 不互相覆盖：另一条错误仍在，文案/count/动作均为自己的',
    !!eOther && eOther.count === 1 && eOther.action.run === runOther);
  checkNew('(e2) 合并后执行的是新动作（不是被替换掉的旧动作）',
    (() => {
      if (!eTimeout?.action?.run) return false;
      eTimeout.action.run();
      return runs2.length === 1 && runs2[0] === 'new';
    })());

  /* ============================================================
     (e2b) 反例 2b（R1 返工核心）：同文案、不同操作实体——显式 key 去重，
     10 条各自可追溯、可分别重试、互不清除（修复「同文案多源失败被合并成
     1 条、重试只重试最新动作」的 P2）
     ============================================================ */
  store.setState({ toasts: [], errors: [] });
  const entityRuns = [];
  const entityKey = (i) => `feed:refresh:${i}`;
  for (let i = 0; i < 10; i += 1) {
    store.getState().showToast('刷新失败：网络不可达', { label: '重试', key: entityKey(i), run: () => entityRuns.push(i) });
  }
  const entity10 = errorsOf();
  checkNew('(e2b) 同文案不同实体：10 条「刷新失败：网络不可达」各自可追溯（显式操作实体 key 不同 → 不合并；修前按文案键会合并成 1 条，其余实体无法各自重试）',
    entity10.length === 10
    && entity10.every((e, i) => e.key === entityKey(i) && e.text === '刷新失败：网络不可达' && e.count === 1));
  checkNew('(e2b) 同文案不同实体逐条可重试：各自动作绑定到自己的实体（执行第 3 条只触发实体 3）',
    (() => {
      if (typeof entity10[3]?.action?.run !== 'function') return false;
      entity10[3].action.run();
      return entityRuns.length === 1 && entityRuns[0] === 3;
    })());
  checkNew('(e2b) 互不清除：消费某一条不误伤其它同文案条目（按实体 key 消费，不是按文案）',
    (() => {
      store.getState().retryError?.(entity10[3]?.id);
      const left = errorsOf();
      return left.length === 9
        && !left.some((e) => e.key === entityKey(3))
        && left.some((e) => e.key === entityKey(2)) && left.some((e) => e.key === entityKey(4));
    })());
  store.setState({ toasts: [], errors: [] });
  const runA = () => {}; const runB = () => {};
  store.getState().showToast('刷新失败：网络不可达', { label: '重试', key: 'feed:refresh:7', run: runA });
  store.getState().showToast('刷新失败：网络不可达', { label: '重试', key: 'feed:refresh:7', run: runB });
  checkNew('(e2b) 同显式 key 合并保留最新动作（同一实体重复失败不堆叠）',
    errorsOf().length === 1 && errorsOf()[0].key === 'feed:refresh:7'
    && errorsOf()[0].count === 2 && errorsOf()[0].action.run === runB);
  store.setState({ toasts: [], errors: [] });
  store.getState().showToast('刷新失败：网络不可达', { label: '重试', run: () => {} }); // 无 key → 回落文案
  store.getState().showToast('刷新失败：网络不可达', { label: '重试', key: 'feed:refresh:9', run: () => {} });
  checkNew('(e2b) 显式 key 与缺省文案键共存：带 key 的实体条目不与无 key 的同文案条目合并（缺省回落语义保留，两条并存）',
    errorsOf().length === 2
    && errorsOf().some((e) => e.key === '刷新失败：网络不可达')
    && errorsOf().some((e) => e.key === 'feed:refresh:9'));

  /* ============================================================
     (e3) 反例 3：重试成功 → 条目消失；失败 → 条目保留（不丢失入口）
     ============================================================ */
  store.setState({ toasts: [], errors: [] });
  let okAttempts = 0;
  /* 真实成功路径形态：成功以普通 toast 收尾、不重报错误（如 refreshOneFeed 的 .then） */
  const okRun = () => { okAttempts += 1; store.getState().showToast('已刷新，新增 2 条'); };
  store.getState().showToast('刷新失败：超时', { label: '重试', run: okRun });
  const okId = errorsOf()[0]?.id;
  store.getState().retryError?.(okId);
  checkNew('(e3) 重试成功（成功路径不重报错误）⇒ 对应条目从错误中心清除，动作恰好执行一次',
    errorsOf().length === 0 && okAttempts === 1
    && store.getState().toasts.at(-1)?.text === '已刷新，新增 2 条');
  checkNew('(e3) 清除后对应 toast 也已不在时，同 id 再点重试为 no-op（条目已消费、无回落目标，不重复执行动作）',
    (() => {
      store.setState({ toasts: [] });
      store.getState().retryError?.(okId);
      return okAttempts === 1;
    })());

  store.setState({ toasts: [], errors: [] });
  let failAttempts = 0;
  /* 真实失败路径形态：catch 里再次 showToast 同 key（ui.ts 各 call site 即此形态） */
  const failRun = () => {
    failAttempts += 1;
    store.getState().showToast('刷新失败：超时', { label: '重试', run: failRun });
  };
  store.getState().showToast('刷新失败：超时', { label: '重试', run: failRun });
  store.getState().retryError?.(errorsOf()[0]?.id);
  const sFail = store.getState();
  checkNew('(e3) 重试再次失败（失败路径重报同一故障）⇒ 条目保留、动作仍是最新的失败动作（入口不丢失）',
    failAttempts === 1 && errorsOf().length === 1
    && errorsOf()[0].key === '刷新失败：超时' && errorsOf()[0].action.run === failRun
    && sFail.toasts.at(-1)?.action?.label === '重试');

  store.setState({ toasts: [], errors: [] });
  store.getState().showToast('清理失败', { label: '重试', run: () => { throw new Error('boom'); } });
  store.getState().retryError?.(errorsOf()[0]?.id);
  checkNew('(e3) 重试动作同步抛错 ⇒ 条目保留（同步失败路径不经过重报，入口不得被吞掉）',
    errorsOf().length === 1 && errorsOf()[0].key === '清理失败');

  /* ---- toast 表面的一键重试与面板同一语义（App 的 ToastPill 经同一动作派发） ---- */
  store.setState({ toasts: [], errors: [] });
  let toastRetry = 0;
  store.getState().showToast('同步失败：超时', { label: '重试', run: () => { toastRetry += 1; } });
  store.getState().retryError?.(store.getState().toasts.at(-1)?.id);
  checkNew('(e3) toast 表面重试同样消费对应条目（两个表示面一致：重试即消费，失败路径重报写回）',
    toastRetry === 1 && errorsOf().length === 0);

  store.setState({ toasts: [], errors: [] });
  const lateRuns = [];
  store.getState().showToast('后台刷新：超时', { label: '重试', run: () => lateRuns.push('旧') });
  const oldToastId = store.getState().toasts.at(-1)?.id;
  store.getState().showToast('后台刷新：超时', { label: '重试', run: () => lateRuns.push('新') }); // 同 key 合并：条目 id 升级
  store.getState().retryError?.(oldToastId); // 点的是旧 toast 上的重试（条目 id 已被合并掉）
  checkNew('(e3) 条目已被同 key 合并（旧 id 不在错误中心）时，从旧 toast 重试按 key 兜底消费合并后的条目（不遗留陈旧入口）',
    lateRuns.length === 1 && lateRuns[0] === '旧' && errorsOf().length === 0);

  /* ============================================================
     (e4) 反例 4：普通 toast 行为不回归 + 退场后条目仍持久可达
     ============================================================ */
  store.setState({ toasts: [], errors: [] });
  store.getState().showToast('新增-普通');
  checkNew('(e4) 普通 toast（无动作）不进入错误中心（瞬时通知语义不变），无 action 载荷',
    errorsOf().length === 0 && store.getState().toasts.length === 1
    && store.getState().toasts[0].action === undefined);
  store.getState().showToast('刷新失败：超时', { label: '重试', run: () => {} });
  await new Promise((r) => setTimeout(r, 2800));
  const sMid = store.getState();
  checkNew('(e4) 2800ms：普通 toast 已退场、带重试 toast 仍在且未进入退场（时长口径不回退）',
    !sMid.toasts.some((t) => t.text === '新增-普通')
    && sMid.toasts.some((t) => t.text === '刷新失败：超时' && t.leaving !== true));
  checkNew('(e4) 此时刻错误中心条目在（toast 还在，入口自然在）', errorsOf().length === 1);
  await new Promise((r) => setTimeout(r, 1900));
  const sEnd = store.getState();
  checkNew('(e4) toast 全部退场后条目仍在：4200ms 后 toast 层清空，错误中心仍保留可重试条目（旧实现在此只剩「等下一条自然触发」）',
    sEnd.toasts.length === 0 && errorsOf().length === 1 && errorsOf()[0].key === '刷新失败：超时');

  /* ============================================================
     (e5) 纯函数真值表：合并/上限/TTL
     ============================================================ */
  const now = 1_700_000_000_000;
  checkNew('(e5) upsert：同 key 替换为最新对象（count/at/动作全取新值）、保留位置语义为「追加到最新」、不改入参数组',
    typeof upsertErrorEntry === 'function' && (() => {
      const base = [mkEntry('a', 1), mkEntry('b', 2)];
      const merged = upsertErrorEntry(base, mkEntry('a', 3, 2), 4);
      return base.length === 2 && base[0].key === 'a' && base[0].at === 1
        && merged.length === 2 && merged[0].key === 'b' && merged[1].key === 'a'
        && merged[1].at === 3 && merged[1].count === 2;
    })());
  checkNew('(e5) upsert：超过 max 时从最旧逐出（保留 tail），条目数与总量无关',
    typeof upsertErrorEntry === 'function'
    && upsertErrorEntry([mkEntry('a', 1), mkEntry('b', 2)], mkEntry('c', 3), 2).map((e) => e.key).join(',') === 'b,c');
  checkNew('(e5) prune：超过 TTL 的条目被清理、未过期保留（生命周期上限）且不改入参',
    typeof pruneErrorEntries === 'function' && (() => {
      const old = mkEntry('旧', now - ERROR_CENTER_TTL_MS - 1);
      const fresh = mkEntry('新', now - 1);
      const base = [old, fresh];
      const out = pruneErrorEntries(base, now);
      return base.length === 2 && out.length === 1 && out[0].key === '新';
    })());

  /* ---- TTL 落到 store：写入时惰性清理 ---- */
  store.setState({ toasts: [], errors: [mkEntry('陈年错误', Date.now() - ERROR_CENTER_TTL_MS - 1000)] });
  store.getState().showToast('新错误', { label: '重试', run: () => {} });
  checkNew('(e6) TTL 落地：超期条目在下一次错误写入时被清出（生命周期上限不是只有常量）',
    errorsOf().length === 1 && errorsOf()[0].key === '新错误');

  /* ============================================================
     (e7) 面板开态动作（App Esc 链与浮层让路清单的字段源）
     ============================================================ */
  store.getState().openErrorCenter?.();
  const opened = store.getState().errorCenterOpen === true;
  store.getState().closeErrorCenter?.();
  checkNew('(e7) open/closeErrorCenter 写 errorCenterOpen（面板开关的单点状态）',
    opened && store.getState().errorCenterOpen === false);
  store.setState({ toasts: [], errors: [] });
  store.getState().dismissError?.(-1);
  checkNew('(e7) dismissError 对不存在的 id 是 no-op（不误删其它条目）', errorsOf().length === 0);

  /* ============================================================
     (e8) 接线（源级反回退）：面板、入口、Esc 链
     ============================================================ */
  const fs = await import('node:fs');
  const appSrc = fs.readFileSync(new URL('../../src/App.tsx', import.meta.url), 'utf8');
  const sideSrc = fs.readFileSync(new URL('../../src/components/Sidebar.tsx', import.meta.url), 'utf8');
  checkNew('(e8) App 渲染错误中心面板且 Esc 链含 errorCenterOpen 分支（浮层可关）',
    appSrc.includes('<ErrorCenterDialog />')
    && /else if \(s\.errorCenterOpen\) s\.closeErrorCenter\(\)/.test(appSrc));
  checkNew('(e8) Sidebar 底部入口仅在 errors 非空时渲染（可关闭：条目清空即入口消失），点击开面板',
    /errors\.length > 0/.test(sideSrc) && sideSrc.includes('sidebar-error-center')
    && sideSrc.includes('openErrorCenter'));
  checkNew('(e8) App 的浮层查询实参传入 errorCenterOpen（清单已登记但调用点漏传即失败）',
    /errorCenterOpen:\s*s\.errorCenterOpen/.test(appSrc));

  /* ============================================================
     (e9) 迁移完整性守卫（R1 返工）：src 下每个 `label: '重试'` 调用点的
     动作对象内必须带显式操作实体 key——否则该错误会回落文案去重，重蹈
     「同文案多实体被合并」。
     守卫为**启发式源级扫描**：从 label 起、到下一个 `label:` 或 240 字符为止
     的窗口内须出现 `key: '…'` / ``key: `…` ``；窗口边界说明见断言名。
     调用点总数钉死：新增/删除重试点必须同步本守卫（与浮层清单同一纪律）。
     ============================================================ */
  const path = await import('node:path');
  const url = await import('node:url');
  const srcDir = url.fileURLToPath(new URL('../../src/', import.meta.url));
  const walk = (dir) => fs.readdirSync(dir, { withFileTypes: true }).flatMap((d) => {
    const p = path.join(dir, d.name);
    return d.isDirectory() ? walk(p) : (/\.(ts|tsx)$/.test(d.name) ? [p] : []);
  });
  const srcFiles = walk(srcDir).map((file) => ({ file, src: fs.readFileSync(file, 'utf8') }));
  const allSrc = srcFiles.map((f) => f.src).join('\n');
  const missingKey = [];
  let retrySites = 0;
  for (const { file, src } of srcFiles) {
    const sites = [...src.matchAll(/label:\s*'重试'/g)];
    if (!sites.length) continue;
    retrySites += sites.length;
    for (const m of sites) {
      const from = m.index + m[0].length;
      const nextLabel = src.indexOf('label:', from + 1);
      const to = nextLabel === -1 ? from + 240 : Math.min(from + 240, nextLabel);
      if (!/key:\s*['`]/.test(src.slice(from, to))) {
        const rel = path.relative(srcDir, file).replaceAll('\\', '/');
        missingKey.push(`${rel}:${src.slice(0, m.index).split('\n').length}`);
      }
    }
  }
  checkNew('(e9) 迁移完整性：全部重试调用点的动作对象窗口（到下一个 label 或 240 字符）内含显式 key（漏 key 即回落文案去重——新重试点必须带 key）',
    retrySites > 0 && missingKey.length === 0, missingKey.join('；'));
  checkNew('(e9) 迁移清单钉死：src 下恰 20 个 `label: \'重试\'` 调用点（新增/删除重试点必须同步本守卫与迁移测试）',
    retrySites === 20);
  checkNew('(e9) key 形态守卫：各操作实体族 key 以「key: \'…\' / key: `…`」字面出现在 src（域:操作:实体；注释里的示例提及不算）',
    ['feed:refresh:', 'feed:add:', 'ai:summary:', 'ai:translate:', 'reader:fulltext:', 'reader:content:', 'list:load-more', 'mark:all-read', 'sync:manual', 'sync:save-and-sync']
      .every((k) => new RegExp(`key:\\s*['\`]${k}`).test(allSrc)));
}
