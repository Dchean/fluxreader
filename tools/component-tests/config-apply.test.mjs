// OPT-011 R5：配置应用/上传经**真实 UI 入口**（ConfigSyncSection 真实按钮）接入
// AI 写协议的真实挂载回归。
//
// 取证方式（与 OPT-016B 同纪律）：挂载真实生产组件 ConfigSyncSection，真实点击
// 「下载并应用 / 上传配置」按钮 → 真实 handler → 真实 store/IPC 假后端（fixtures）：
// - 下载并应用：用户确认后进入受控写屏障——等待**进入时已存在**的写前沿（含
//   排队的编辑 composite），应用期间的 AI 点击乐观可见、请求排在应用之后；应用
//   成功后以真实 list_folders/list_feeds 回读确认（不把可能被查询代际淘汰的
//   reload 当已确认）；取消不进入屏障、失败释放且错误可见。
// - 上传：等待发起时已存在的写前沿（快照不漏已点写）；网络往返不设门——之后
//   的 AI 操作照常发出，不被上传阻塞。
// - 回读失败：如实告知未确认（后续失败不得声称「已恢复原值」）。
// 未预期 console/jsdom/React 错误由 componentTest 统一账本终检判红。

import assert from 'node:assert/strict';
import { componentTest, mount, unmount, flush, waitFor, resetStore, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, clearInvokeCalls, invokeCount, setPlan, resetPlan, FOLDERS, FEEDS } from './fixtures.mjs';

const ROWS = [mkRow({ id: 101, feed_id: 10, title: '文章一' })];

function deepCopy(x) {
  return JSON.parse(JSON.stringify(x));
}

/** 结算受控挂起项直到静默：链路反应（队列继续发下一笔/屏障转入下一阶段）可能
    产生新的挂起项——多轮结算；失败路径也不例外，绝不把写屏障或队列留给后续
    用例（模块级 writeGate/queue 跨用例存活）。 */
async function drainControlled(act, pools) {
  for (let round = 0; round < 8; round++) {
    if (pools.every((p) => p.items.length === 0)) return;
    await act(async () => {
      for (const p of pools) {
        for (const item of p.items.splice(0)) {
          if (p.kind === 'reject') item.reject({ message: 'teardown' });
          else item.resolve(p.value === undefined ? null : p.value);
        }
      }
      await new Promise((r) => setTimeout(r, 0));
    });
  }
}

/** 挂载 ConfigSyncSection（真实按钮随之可查）；不等待状态回填——
    状态响应由用例经 enableSection 在 act 内放行。 */
async function mountSection() {
  const { createElement } = await import('react');
  const { ConfigSyncSection } = await import('../../src/components/settings/ConfigSyncSection.tsx');
  const entry = await mount(createElement(ConfigSyncSection));
  await flush(1);
  const buttons = [...entry.container.querySelectorAll('button')];
  const uploadBtn = buttons.find((b) => b.textContent.includes('上传配置'));
  const applyBtn = buttons.find((b) => b.textContent.includes('下载并应用'));
  assert.ok(uploadBtn && applyBtn, '两个真实按钮存在（上传配置 / 下载并应用）');
  return { entry, uploadBtn, applyBtn };
}

/** 在 act 内放行受控的 config_sync_status 响应并等按钮就绪。
    组件的挂载期 effect 是 `configSyncStatus().then(setStatus)`——响应若用立即
    resolve 的 Promise，回填会落在 act 窗口之外（错误账本判红，且随机器时序
    时红时绿）。受控 deferred + act 内放行让该 setStatus 确定性地落在 act 内，
    与全套件「受控 Promise」纪律一致（不关账本、不加例外）。 */
async function enableSection(act, resolveStatus, applyBtn) {
  await act(async () => { resolveStatus({ configured: true, backend: 'gist' }); });
  await flush(2);
  await waitFor(() => !applyBtn.disabled, { label: '状态就绪后应用按钮可用' });
}

componentTest('OPT-011 R5：下载并应用——确认后屏障等全前沿（开关+排队编辑）、期间新点击门控、真实回读确认', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  /* 独立持久态 PF + 冻结读取副本；写提交（改 PF）与回应分开控制。 */
  const PF = { folders: deepCopy(FOLDERS), feeds: deepCopy(FEEDS) };
  const writes = [];            // set_feed_ai_modes 挂起项
  const catWrites = [];         // set_folder_ai_flags 挂起项
  const updates = [];           // update_feed（编辑 composite）挂起项
  const applies = [];           // config_sync_apply 挂起项
  const pendingArticles = [];   // 全量 reload 的 list_articles 挂起（证明确认不依赖 reload 落地）
  let resolveStatus = null;     // 受控 config_sync_status（act 内放行，见 enableSection）
  setPlan({
    config_sync_status: () => new Promise((resolve) => { resolveStatus = resolve; }),
    config_sync_download: () => Promise.resolve(JSON.stringify({ uploaded_at: '2026-10-10T00:00:00Z', feeds: [{ url: 'https://x.example/rss', title: 'X' }] })),
    config_sync_apply: () => new Promise((resolve, reject) => applies.push({ resolve, reject })),
    list_folders: () => Promise.resolve(deepCopy(PF.folders)),
    list_feeds: () => Promise.resolve(deepCopy(PF.feeds)),
    list_articles: () => new Promise((resolve) => pendingArticles.push({ resolve })),
    set_feed_ai_modes: (args) => new Promise((resolve, reject) => writes.push({ args, resolve, reject })),
    set_folder_ai_flags: (args) => new Promise((resolve, reject) => catWrites.push({ args, resolve, reject })),
    update_feed: (args) => new Promise((resolve, reject) => updates.push({ args, resolve, reject })),
  });
  const origConfirm = window.confirm;
  const confirmMsgs = [];
  window.confirm = (msg) => { confirmMsgs.push(String(msg)); return true; };
  const { act } = await import('react');
  try {
    const { entry, applyBtn } = await mountSection();
    await enableSection(act, resolveStatus, applyBtn);

    /* 前沿①：快捷开关写挂起（已提交未回包） */
    await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'on'); });
    await flush(2);
    assert.equal(writes.length, 1, '前置：开关写挂起');
    /* 前沿②：编辑 composite 排队在开关之后（update_feed 未发出） */
    await act(async () => {
      useAppStore.getState().editFeed('10', { title: '', catId: 'cat-1', layout: 'inherit', summaryMode: 'off', translateMode: 'off' });
    });
    await flush(2);
    assert.equal(invokeCount('update_feed'), 0, '前置：编辑排在开关之后未发出（同一写协议入队）');

    /* 真实点击「下载并应用」→ 确认 → 屏障等待全前沿：apply 未发出 */
    await act(async () => { applyBtn.click(); });
    await flush(3);
    assert.equal(confirmMsgs.length, 1, '真实确认框被调用');
    assert.equal(invokeCount('config_sync_apply'), 0, '屏障等待已存在的前沿（开关 + 排队编辑）：apply 尚未发出');

    /* 屏障期间新点击（不同键 → 纯门控证明）：乐观当帧可见，请求排在应用之后 */
    await act(async () => { useAppStore.getState().toggleCatTranslate('cat-1', true); });
    await flush(2);
    assert.equal(useAppStore.getState().feedIndex.get('10')?.cat.autoTranslate, true, '应用期间新点击乐观可见');
    assert.equal(catWrites.length, 0, '新写被门控：应用完成前不发出');

    /* 放行开关写 → 编辑才发出；apply 仍在等编辑落定（前沿覆盖排队编辑） */
    await act(async () => { writes[0].resolve(null); });
    await flush(3);
    assert.equal(updates.length, 1, '开关落定后编辑发出（同域 wire 顺序：开关 → 编辑）');
    assert.equal(invokeCount('config_sync_apply'), 0, '编辑在途：屏障继续等待（前沿=整条已存在写链）');

    /* 放行编辑（提交 off）→ apply 才发出 */
    PF.feeds.find((f) => f.id === 10).summary_mode = 'off';
    await act(async () => { updates[0].resolve(null); });
    await flush(3);
    assert.equal(invokeCount('config_sync_apply'), 1, '编辑落定后 apply 才发出');
    assert.equal(catWrites.length, 0, '应用在途：新写仍被门控');

    /* 应用提交：远端导入把摘要旗标改为 on（与本地编辑值 off 不同——覆盖语义） */
    PF.feeds.find((f) => f.id === 10).summary_mode = 'on';
    await act(async () => { applies[0].resolve({ imported: 1, updated: 0, skipped: 0 }); });
    await flush(3);

    /* 真实回读确认：确认态/可视值 = 后端导入值；不依赖 reload（其 list_articles 仍挂起） */
    assert.ok(pendingArticles.length >= 1, '前置：全量 reload 仍在途（list_articles 挂起，未被当确认依据）');
    const { selectFeedConfig } = await import('../../dist-test/store/selectors.js');
    assert.equal(selectFeedConfig(useAppStore.getState(), '10').summaryMode, 'on',
      '回读接受导入值 on（本地编辑 off 被应用覆盖、界面与后端一致）');
    assert.equal(invokeCount('list_feeds') >= 1, true, 'apply 成功后走真实配置回读（list_feeds）');
    /* 屏障释放：应用期间的新点击才发出 */
    assert.equal(catWrites.length, 1, '屏障释放后，应用期间的新写发出（乐观意图不被吞）');
    await act(async () => { catWrites[0].resolve(null); });
    await flush(3);
    assert.equal(useAppStore.getState().feedIndex.get('10')?.cat.autoTranslate, true, '新写成功落定，意图保持');
    assert.ok(useAppStore.getState().toasts.some((t) => t.text.startsWith('配置已应用')), '应用结果如实提示');
    assert.ok(entry.container.querySelector('.settings-group-title'), '组件挂载树保持');

    /* 放行挂起的 reload（收尾） */
    await act(async () => { for (const r of pendingArticles.splice(0)) r.resolve([]); });
    await flush(2);
  } finally {
    window.confirm = origConfirm;
    /* 防悬挂（失败路径也不把写屏障/队列留给后续用例）：多轮结算全部受控挂起项。 */
    await drainControlled(act, [
      { kind: 'resolve', items: writes },
      { kind: 'resolve', items: catWrites },
      { kind: 'reject', items: updates },
      { kind: 'reject', items: applies },
      { kind: 'resolve', items: pendingArticles, value: [] },
    ]);
    await flush(1);
    resetPlan();
  }
});

componentTest('OPT-011 R5：取消不进入屏障不残留；应用失败释放门且错误可见', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const applies = [];
  let confirmAnswer = false;
  let resolveStatus = null;
  setPlan({
    config_sync_status: () => new Promise((resolve) => { resolveStatus = resolve; }),
    config_sync_download: () => Promise.resolve(JSON.stringify({ uploaded_at: 'x', feeds: [] })),
    config_sync_apply: () => new Promise((resolve, reject) => applies.push({ resolve, reject })),
  });
  const origConfirm = window.confirm;
  window.confirm = () => confirmAnswer;
  const { act } = await import('react');
  try {
    const { applyBtn } = await mountSection();
    await enableSection(act, resolveStatus, applyBtn);

    /* 用户取消：不执行应用、不进入屏障（后续 AI 写照常立即发出） */
    await act(async () => { applyBtn.click(); });
    await flush(3);
    assert.equal(invokeCount('config_sync_apply'), 0, '取消后不执行应用');
    await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'on'); });
    await flush(2);
    assert.equal(invokeCount('set_feed_ai_modes'), 1, '取消不残留屏障：后续写照常立即发出');
    assert.equal(useAppStore.getState().feedIndex.get('10')?.feed.summaryMode, 'on', '写入乐观可见');

    /* 确认应用：屏障期间新点击被门控；应用失败 → 释放 + 错误可见 */
    confirmAnswer = true;
    await act(async () => { applyBtn.click(); });
    await flush(3);
    assert.equal(applies.length, 1, '确认后 apply 发出（无前沿，立即执行）');
    await act(async () => { useAppStore.getState().toggleCatTranslate('cat-1', true); });
    await flush(2);
    assert.equal(invokeCount('set_folder_ai_flags'), 0, '应用在途：新写被门控');
    await act(async () => { applies[0].reject({ message: '注入：应用失败' }); });
    await flush(3);
    assert.ok(useAppStore.getState().toasts.some((t) => t.text.startsWith('下载失败：注入：应用失败')), '失败错误可见（不吞）');
    assert.equal(invokeCount('set_folder_ai_flags'), 1, '失败释放门：门控写随即发出');
  } finally {
    window.confirm = origConfirm;
    await drainControlled(act, [{ kind: 'reject', items: applies }]);
    await flush(1);
    resetPlan();
  }
});

componentTest('OPT-011 R5：上传等待发起时写前沿；网络往返不阻塞后续 AI 操作', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const writes = [];
  const catWrites = [];
  const uploads = [];
  let resolveStatus = null;
  setPlan({
    config_sync_status: () => new Promise((resolve) => { resolveStatus = resolve; }),
    config_sync_upload: () => new Promise((resolve, reject) => uploads.push({ resolve, reject })),
    set_feed_ai_modes: (args) => new Promise((resolve, reject) => writes.push({ args, resolve, reject })),
    set_folder_ai_flags: (args) => new Promise((resolve, reject) => catWrites.push({ args, resolve, reject })),
  });
  const { act } = await import('react');
  try {
    const { uploadBtn, applyBtn } = await mountSection();
    await enableSection(act, resolveStatus, applyBtn);

    /* 发起时已有写前沿：上传等待其落定（快照不漏已点写） */
    await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'on'); });
    await flush(2);
    assert.equal(writes.length, 1, '前置：AI 写挂起');
    await act(async () => { uploadBtn.click(); });
    await flush(3);
    assert.equal(invokeCount('config_sync_upload'), 0, '上传等待发起时已存在的写前沿：尚未发出');

    /* 之后的 AI 点击不被上传（前沿等待/网络往返）阻塞：不同键立即发出 */
    await act(async () => { useAppStore.getState().toggleCatTranslate('cat-1', true); });
    await flush(2);
    assert.equal(catWrites.length, 1, '后续 AI 操作照常发出（上传不是全局锁）');

    /* 放行前沿写 → 上传才发出；上传在途期间再点击仍不被阻塞 */
    await act(async () => { writes[0].resolve(null); });
    await flush(3);
    assert.equal(invokeCount('config_sync_upload'), 1, '前沿落定后上传发出');
    await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'off'); });
    await flush(2);
    assert.equal(writes.length, 2, '上传网络往返期间：新 AI 写照常发出（无写门）');

    await act(async () => {
      catWrites[0].resolve(null);
      uploads[0].resolve('2026-10-10T01:00:00Z');
    });
    await flush(3);
    assert.ok(useAppStore.getState().toasts.some((t) => t.text === '配置已上传'), '上传成功提示如实');
    await act(async () => { writes[1].resolve(null); });
    await flush(2);
  } finally {
    await drainControlled(act, [
      { kind: 'resolve', items: writes },
      { kind: 'resolve', items: catWrites },
      { kind: 'resolve', items: uploads, value: 'teardown' },
    ]);
    await flush(1);
    resetPlan();
  }
});

componentTest('OPT-011 R5：应用成功但回读失败——如实告知未确认、后续失败不假称已恢复', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const writes = [];
  let resolveStatus = null;
  setPlan({
    config_sync_status: () => new Promise((resolve) => { resolveStatus = resolve; }),
    config_sync_download: () => Promise.resolve(JSON.stringify({ uploaded_at: 'x', feeds: [] })),
    config_sync_apply: () => Promise.resolve({ imported: 0, updated: 0, skipped: 0 }),
    list_feeds: () => Promise.reject({ message: '注入：读失败' }),
    set_feed_ai_modes: (args) => new Promise((resolve, reject) => writes.push({ args, resolve, reject })),
  });
  const origConfirm = window.confirm;
  window.confirm = () => true;
  const { act } = await import('react');
  try {
    const { applyBtn } = await mountSection();
    await enableSection(act, resolveStatus, applyBtn);

    /* 先建立该键的协议状态（成功一笔：确认为 on） */
    await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'on'); });
    await flush(2);
    await act(async () => { writes[0].resolve(null); });
    await flush(2);

    /* 应用成功但回读失败（list_feeds 拒绝） */
    await act(async () => { applyBtn.click(); });
    await flush(4);
    assert.ok(useAppStore.getState().toasts.some((t) => t.text.includes('配置已应用，但配置回读失败')),
      '回读失败如实告知未确认（不假称已对齐）');

    /* 后续写入失败：确认态不可信 → 只提示未保存，不声称「已恢复原值」 */
    await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'off'); });
    await flush(2);
    assert.equal(writes.length, 2, '回读失败不粘滞队列：后续写照常发出（门已释放）');
    await act(async () => { writes[1].reject({ message: '注入：写失败' }); });
    await flush(3);
    assert.ok(useAppStore.getState().toasts.some((t) => t.text.includes('未能保存') && !t.text.includes('已恢复原值')),
      '未确认键的失败提示不假称已恢复');
    assert.ok(!useAppStore.getState().toasts.some((t) => t.text.includes('已恢复原值')),
      '整个回读失败周期内不出现「已恢复原值」虚假声明');
  } finally {
    window.confirm = origConfirm;
    await drainControlled(act, [{ kind: 'resolve', items: writes }]);
    await flush(1);
    resetPlan();
  }
});

/* ============================================================
   OPT-011 R6：多屏障（卸载重挂载并发双应用）
   缺陷（R5 初审被驳回的前提）：组件的 busy 是**本地 state**，SettingsModal
   按 tab 条件卸载 SyncTab——应用 A 在途时切走再回来 busy 已复位，可再发 B。
   旧屏障是单槽 + 无条件清空：B 覆盖 writeGate，A 完成为 null 时把 B 的门
   也打开——B 仍在途时新 AI 写漏过门、被整包应用覆盖。
   修后：屏障自身串行（等前一屏障 + 当时前沿）、门清空有所有权（只在尾是
   自己时清空）、own gate 成功/失败都始终释放。下列用例用**真实卸载/重挂载**
   + 独立持久态与冻结回读取证（不只 assert pending 数）。
   ============================================================ */

componentTest('OPT-011 R6：卸载重挂载并发双应用——屏障串行、A 完成不打开 B 的门、写排 B 确认后', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const PF = { folders: deepCopy(FOLDERS), feeds: deepCopy(FEEDS) };
  const applies = [];   // config_sync_apply 挂起项（A/B 共用受控池）
  const writes = [];    // set_feed_ai_modes 挂起项
  let resolveStatus = null;
  setPlan({
    config_sync_status: () => new Promise((resolve) => { resolveStatus = resolve; }),
    config_sync_download: () => Promise.resolve(JSON.stringify({ uploaded_at: 'x', feeds: [] })),
    config_sync_apply: () => new Promise((resolve, reject) => applies.push({ resolve, reject })),
    list_folders: () => Promise.resolve(deepCopy(PF.folders)),
    list_feeds: () => Promise.resolve(deepCopy(PF.feeds)),
    set_feed_ai_modes: (args) => new Promise((resolve, reject) => writes.push({ args, resolve, reject })),
  });
  const origConfirm = window.confirm;
  window.confirm = () => true;
  const { act } = await import('react');
  try {
    /* A：真实挂载 → 应用挂起（屏障 A 在途） */
    const a = await mountSection();
    await enableSection(act, resolveStatus, a.applyBtn);
    await act(async () => { a.applyBtn.click(); });
    await flush(3);
    assert.equal(invokeCount('config_sync_apply'), 1, '前置：A 的应用已发出并挂起');

    /* 用户切 tab（真实卸载，busy 随组件消失）→ 重挂载 B → 再应用 */
    await unmount(a.entry);
    const b = await mountSection();
    await enableSection(act, resolveStatus, b.applyBtn);
    await act(async () => { b.applyBtn.click(); });
    await flush(3);
    assert.equal(invokeCount('config_sync_apply'), 1,
      'B 的屏障等待 A 完成：B 的 apply 仍未发出（修前单槽被覆盖/不串行，B 会立即发出）');

    /* 放行 A：B 才起步（串行）；A 的真实回读落地 */
    PF.feeds.find((f) => f.id === 10).summary_mode = 'on';
    await act(async () => { applies[0].resolve({ imported: 1, updated: 0, skipped: 0 }); });
    await flush(3);
    assert.equal(invokeCount('config_sync_apply'), 2, 'A 完成后 B 的应用才发出（屏障串行）');
    const { selectFeedConfig } = await import('../../dist-test/store/selectors.js');
    assert.equal(selectFeedConfig(useAppStore.getState(), '10').summaryMode, 'on',
      'A 的真实回读已落地（独立持久态，非 pending 计数）');

    /* A 已完成、B 仍在途：此时发出的快捷 AI 写——修前 A 的 finally 会误清 B 的门 */
    await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'off'); });
    await flush(2);
    assert.equal(useAppStore.getState().feedIndex.get('10')?.feed.summaryMode, 'off', '快捷写乐观可见');
    assert.equal(invokeCount('set_feed_ai_modes'), 0,
      'A 完成不得打开 B 的门：B 在途期间的快捷写不发出（修前此处漏过门）');

    /* 放行 B（导入改标题，验证 B 的回读也落地）——写必须排在 B 确认之后 */
    PF.feeds.find((f) => f.id === 10).title = 'B 导入标题';
    await act(async () => { applies[1].resolve({ imported: 0, updated: 1, skipped: 0 }); });
    await flush(3);
    assert.equal(invokeCount('set_feed_ai_modes'), 1, 'B 完成后快捷写才发出（写排在 B 确认之后）');
    assert.equal(useAppStore.getState().feedIndex.get('10')?.feed.name, 'B 导入标题',
      'B 的真实回读已落地（标题来自独立持久态）');
    await act(async () => { writes[0].resolve(null); });
    await flush(3);
    assert.equal(useAppStore.getState().feedIndex.get('10')?.feed.summaryMode, 'off', '写成功落定，意图保持');
    assert.ok(useAppStore.getState().toasts.some((t) => t.text.startsWith('配置已应用')), 'B 侧应用结果如实提示');
    assert.ok(b.entry.container.querySelector('.settings-group-title'), 'B 挂载树保持');
  } finally {
    window.confirm = origConfirm;
    await drainControlled(act, [
      { kind: 'reject', items: applies },
      { kind: 'resolve', items: writes },
    ]);
    await flush(1);
    resetPlan();
  }
});

componentTest('OPT-011 R6：A 失败且两屏障之间有 AI 写——写等 A、B 等写、A 不等 B（无自锁）', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const PF = { folders: deepCopy(FOLDERS), feeds: deepCopy(FEEDS) };
  const applies = [];
  const writes = [];
  let resolveStatus = null;
  setPlan({
    config_sync_status: () => new Promise((resolve) => { resolveStatus = resolve; }),
    config_sync_download: () => Promise.resolve(JSON.stringify({ uploaded_at: 'x', feeds: [] })),
    config_sync_apply: () => new Promise((resolve, reject) => applies.push({ resolve, reject })),
    list_folders: () => Promise.resolve(deepCopy(PF.folders)),
    list_feeds: () => Promise.resolve(deepCopy(PF.feeds)),
    set_feed_ai_modes: (args) => new Promise((resolve, reject) => writes.push({ args, resolve, reject })),
  });
  const origConfirm = window.confirm;
  window.confirm = () => true;
  const { act } = await import('react');
  try {
    /* A 挂起 */
    const a = await mountSection();
    await enableSection(act, resolveStatus, a.applyBtn);
    await act(async () => { a.applyBtn.click(); });
    await flush(3);
    assert.equal(invokeCount('config_sync_apply'), 1, '前置：A 的应用挂起');

    /* 两屏障之间排一笔 AI 写：捕获 A 的门（B 尚未发布） */
    await act(async () => { useAppStore.getState().toggleFeedSummary('cat-1', '10', 'on'); });
    await flush(2);
    assert.equal(invokeCount('set_feed_ai_modes'), 0, '前置：写等 A 的门，未发出');

    /* 卸载 A → 重挂载 B → 再应用：B 等 [A 的门 + 当时前沿（含这笔写的尾）] */
    await unmount(a.entry);
    const b = await mountSection();
    await enableSection(act, resolveStatus, b.applyBtn);
    await act(async () => { b.applyBtn.click(); });
    await flush(3);
    assert.equal(invokeCount('config_sync_apply'), 1, 'B 等待 A 与两屏障间的写：apply 未发出');

    /* A 失败：own gate 仍释放（不吞错、不打开 B 的门）——两屏障间的写被放行 */
    await act(async () => { applies[0].reject({ message: '注入：A 应用失败' }); });
    await flush(3);
    assert.ok(useAppStore.getState().toasts.some((t) => t.text === '下载失败：注入：A 应用失败'),
      'A 失败如实可见（不吞）');
    assert.equal(invokeCount('set_feed_ai_modes'), 1, 'A 失败也释放门：两屏障间的写发出');
    assert.equal(invokeCount('config_sync_apply'), 1, '写未落定：B 仍在等（等的是进入时前沿——无自锁的拓扑序 A→W→B）');

    /* 写落定 → B 才执行 */
    await act(async () => { writes[0].resolve(null); });
    await flush(3);
    assert.equal(invokeCount('config_sync_apply'), 2, '写落定后 B 才执行（B 等写、写等 A、A 不等 B）');
    await act(async () => { applies[1].resolve({ imported: 0, updated: 0, skipped: 0 }); });
    await flush(3);
    assert.ok(b.entry.container.querySelector('.settings-group-title'), 'B 挂载树保持');
  } finally {
    window.confirm = origConfirm;
    await drainControlled(act, [
      { kind: 'reject', items: applies },
      { kind: 'resolve', items: writes },
    ]);
    await flush(1);
    resetPlan();
  }
});

/* ============================================================
   OPT-008B R3（P2-N3）：配置应用/上传的 **app_settings** 接线的真实挂载回归。
   - 应用期间新点击：乐观可见、请求排在应用+回读之后、载荷发送时重建；
     回读保留点击键（不丢点击）、其余键取远端；确认基线以后端为准；
   - 上传等待发起时的设置写前沿（快照不漏已点写），且不设门——网络往返
     不阻塞后续设置写。
   ============================================================ */

componentTest('OPT-008B R3：下载并应用接入设置边界——新点击排应用后、回读保留点击键、确认基线刷新', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });
  const { resetAppSettingsWriteStateForTests } = await import('../../dist-test/store/slices/settings.js');
  resetAppSettingsWriteStateForTests();

  const applies = [];        // config_sync_apply 挂起项
  const settingsWrites = []; // set_setting 挂起项
  const settingsReads = [];  // get_setting 挂起项（应用回读）
  let failNextSetting = false;
  const REMOTE_SETTINGS = JSON.stringify({ smartDedup: true, fontSize: 22 });
  let resolveStatus = null;
  setPlan({
    config_sync_status: () => new Promise((resolve) => { resolveStatus = resolve; }),
    config_sync_download: () => Promise.resolve(JSON.stringify({ uploaded_at: 'x', feeds: [] })),
    config_sync_apply: () => new Promise((resolve, reject) => { applies.push({ resolve, reject }); }),
    get_setting: (args) => new Promise((resolve) => { settingsReads.push({ args, resolve }); }),
    set_setting: (args) => {
      if (failNextSetting) {
        failNextSetting = false;
        return Promise.reject({ message: '注入：设置保存失败' });
      }
      return new Promise((resolve, reject) => { settingsWrites.push({ args, resolve, reject }); });
    },
  });
  const origConfirm = window.confirm;
  window.confirm = () => true;
  const { act } = await import('react');
  try {
    const { entry, applyBtn } = await mountSection();
    await enableSection(act, resolveStatus, applyBtn);

    /* 真实点击「下载并应用」→ 应用发出（无既有前沿） */
    await act(async () => { applyBtn.click(); });
    await flush(3);
    assert.equal(invokeCount('config_sync_apply'), 1, '应用已发出');
    assert.equal(useAppStore.getState().settings.smartDedup, false, '前置：应用前本地 dedup=false');

    /* 应用在途：新点击乐观可见、请求被门控（排到应用+回读之后） */
    await act(async () => { useAppStore.getState().updateSettings({ fontSize: 18 }); });
    await flush(2);
    assert.equal(useAppStore.getState().settings.fontSize, 18, '应用期间新点击乐观可见');
    assert.equal(settingsWrites.length, 0, '点击请求被门控：应用+回读完成前不发出');

    /* 应用提交 → AI 回读（list_folders/list_feeds 默认即时）→ 设置回读挂起 */
    await act(async () => { applies[0].resolve({ imported: 0, updated: 0, skipped: 0 }); });
    await flush(4);
    assert.equal(settingsReads.length, 1, '应用成功后走真实 app_settings 回读（get_setting）');
    assert.equal(settingsWrites.length, 0, '回读在途：点击请求仍被门控（排到回读之后）');

    /* 放行回读（远端：dedup=true / fontSize=22）→ 点击键保留、其余取远端 */
    await act(async () => { settingsReads[0].resolve(REMOTE_SETTINGS); });
    await flush(4);
    assert.equal(useAppStore.getState().settings.smartDedup, true, '回读生效：远端 dedup=true 落地');
    assert.equal(useAppStore.getState().settings.fontSize, 18, '回读保留点击键（fontSize 未被远端 22 覆盖，点击不丢）');
    assert.equal(settingsWrites.length, 1, '门释放后点击请求才发出（排到应用+回读之后）');
    const sent = JSON.parse(settingsWrites[0].args.value);
    assert.equal(sent.fontSize, 18, '点击载荷发送时重建：含点击值');
    assert.equal(sent.smartDedup, true, '点击载荷发送时重建：含回读后的远端基线（不把远端覆盖打回）');

    /* 点击落库成功 → 确认基线 = 该载荷；随后一笔失败回滚到它（而非应用前旧值） */
    await act(async () => { settingsWrites[0].resolve(null); });
    await flush(2);
    failNextSetting = true;
    await act(async () => { useAppStore.getState().updateSettings({ themeMode: 'light' }); });
    await flush(3);
    assert.equal(useAppStore.getState().settings.themeMode, 'dark', '失败回滚到确认基线（themeMode=dark）');
    assert.equal(useAppStore.getState().settings.smartDedup, true, '确认基线已刷新为回读后的远端值（dedup=true 不被回滚）');
    assert.equal(useAppStore.getState().settings.fontSize, 18, '确认基线含点击值（fontSize=18 保持）');
    assert.ok(useAppStore.getState().toasts.some((t) => t.text.includes('设置保存失败')), '失败如实可见');
    assert.ok(entry.container.querySelector('.settings-group-title'), '组件挂载树保持');
  } finally {
    window.confirm = origConfirm;
    await drainControlled(act, [
      { kind: 'reject', items: applies },
      { kind: 'resolve', items: settingsReads, value: null },
      { kind: 'reject', items: settingsWrites },
    ]);
    await flush(1);
    resetPlan();
    resetAppSettingsWriteStateForTests();
  }
});

componentTest('OPT-008B R3：上传等待发起时的设置写前沿；往返不阻塞后续设置写', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });
  const { resetAppSettingsWriteStateForTests } = await import('../../dist-test/store/slices/settings.js');
  resetAppSettingsWriteStateForTests();

  const uploads = [];
  const settingsWrites = [];
  let resolveStatus = null;
  setPlan({
    config_sync_status: () => new Promise((resolve) => { resolveStatus = resolve; }),
    config_sync_upload: () => new Promise((resolve, reject) => { uploads.push({ resolve, reject }); }),
    set_setting: (args) => new Promise((resolve, reject) => { settingsWrites.push({ args, resolve, reject }); }),
  });
  const { act } = await import('react');
  try {
    const { uploadBtn, applyBtn } = await mountSection();
    await enableSection(act, resolveStatus, applyBtn);

    /* 发起时已有设置写前沿：上传须等其落定（快照不漏已点写） */
    await act(async () => { useAppStore.getState().updateSettings({ fontSize: 18 }); });
    await flush(2);
    assert.equal(settingsWrites.length, 1, '前置：设置写挂起');
    await act(async () => { uploadBtn.click(); });
    await flush(3);
    assert.equal(uploads.length, 0, '上传等待发起时已存在的设置写前沿：尚未发出');

    /* 前沿落定 → 上传发出；其网络往返不设门：后续设置写照常发出 */
    await act(async () => { settingsWrites[0].resolve(null); });
    await flush(3);
    assert.equal(uploads.length, 1, '前沿落定后上传才发出');
    await act(async () => { useAppStore.getState().updateSettings({ fontSize: 20 }); });
    await flush(2);
    assert.equal(settingsWrites.length, 2, '上传往返期间：新设置写照常发出（无写门）');

    await act(async () => { uploads[0].resolve('2026-10-12T00:00:00Z'); });
    await flush(3);
    assert.ok(useAppStore.getState().toasts.some((t) => t.text === '配置已上传'), '上传成功提示如实');
    await act(async () => { settingsWrites[1].resolve(null); });
    await flush(2);
  } finally {
    await drainControlled(act, [
      { kind: 'reject', items: uploads },
      { kind: 'reject', items: settingsWrites },
    ]);
    await flush(1);
    resetPlan();
    resetAppSettingsWriteStateForTests();
  }
});
