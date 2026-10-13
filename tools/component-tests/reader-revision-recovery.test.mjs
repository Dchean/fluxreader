// OPT-008B R3（主控清单①）：Reader 主动修订恢复的真实组件回归。
//
// 为什么必须挂真实组件：R2 复审 P3-1 指出——正文修订失效（invalidateContentRevision
// 清空记录回 cleared）后的重水合依赖被挂载列表卡片的 useLazyHydrate；虚拟列表中
// 卡片滚出挂载窗时没有任何人入队，打开中的文章会停在空正文直到用户交互。
// R3 在 Reader 加了对 activeArticleId「cleared 且正文空」的幂等 ensureArticleContent
// 观察。本用例只挂真实 Reader、**完全不挂 Timeline**（没有任何卡片，useLazyHydrate
// 不可能参与），在真实 IPC 边界（fixtures 假后端，数据形状/时机可控）上取证：
//   1. 初始水合 → Reader 渲染修订一；
//   2. 后端内容更新（revision 推进）→ reload 失效 → Reader **自动**重取并渲染
//      修订二（无卡片、无用户交互）；
//   3. 在途旧响应场景：记录在途时再推进修订 → 失效清空；Reader 自动重新入队；
//      放行**旧戳**响应被整体丢弃（不复活），新响应落地渲染修订三。
// 断言同时读 DOM（.article-prose）与 dist-test bodyCache 记录（同一实例）。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, getBodyEntry, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, setCurrentRows, setPlan, deferred, clearInvokeCalls, invokeCalls } from './fixtures.mjs';

const ROW = (revision, html) => mkRow({
  id: 201,
  feed_id: 11,
  title: '修订恢复 201',
  snippet: 's201',
  url: 'https://example.com/201',
  content_id: 7001,
  content_revision: revision,
  content_html: html,
});

const getArticlesCalls = () => invokeCalls.filter((c) => c.cmd === 'get_articles').length;

componentTest('OPT-008B R3 Reader 修订恢复：无 Timeline 卡片也能自动重水合；在途旧响应被丢弃后新响应落地', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();

  const rows1 = [ROW(1, '<p>201 修订一</p>')];
  seedFromRows(rows1, { layout: 'social' });
  setCurrentRows(rows1);

  const { createElement, act } = await import('react');
  const { Reader } = await import('../../src/components/Reader.tsx');
  const entry = await mount(createElement(Reader));
  await flush(3);

  /* 打开文章：显式置 activeArticleId（不挂 Timeline，无任何卡片节点）。 */
  await act(async () => { useAppStore.setState({ activeArticleId: '201' }); });
  await flush(2);
  assert.equal(entry.container.querySelectorAll('.article-card, .social-card, .notif-card, .gallery-card, .podcast-card').length, 0,
    '前置：本用例不挂任何列表卡片（重水合不能由 useLazyHydrate 承担）');

  /* 初始水合：get_articles 手动放行（真实在途窗口）。 */
  const batches = [];
  setPlan({
    get_articles: (args) => {
      const d = deferred();
      batches.push({ ids: (args?.ids ?? []).map(String), d });
      return d.promise;
    },
  });
  await act(async () => { useAppStore.getState().hydrateArticleContent(['201']); });
  await flush(2);
  assert.equal(batches.length, 1, '初始水合发出一次 get_articles');
  await act(async () => { batches.at(-1).d.resolve(rows1); });
  await flush(3);
  assert.equal(getBodyEntry('201')?.state, 'ready');
  assert.ok(entry.container.querySelector('.article-prose')?.textContent.includes('201 修订一'),
    'Reader 渲染修订一（真实 DOM 取证）');

  /* ---- 场景 1：后端内容更新（revision 推进）→ reload 失效 → Reader 自动重取 ---- */
  const rows2 = [ROW(2, '<p>201 修订二 后半段更新</p>')];
  setCurrentRows(rows2);
  await act(async () => { await useAppStore.getState().reloadFromBackend(); });
  /* 失效与 Reader 自动重入队都发生在同一次 act 内（effect 紧接快照落地），
     因此这里断言「旧正文已清空且不在 ready」而不是要求捕捉 cleared 瞬时态；
     失效的具体状态由 store 级 t008b 与最终自动恢复闭环共同锁定。 */
  const afterRevBump = getBodyEntry('201');
  assert.ok(afterRevBump?.state !== 'ready', `revision 推进 → 记录不得停留 ready（实际 ${afterRevBump?.state}）`);
  assert.equal(afterRevBump?.content, '', '失效后正文清空（不停留旧正文）');
  await waitFor(() => batches.length >= 2, { label: 'Reader 自动重取（无卡片、无用户交互）', timeout: 8000 });
  assert.deepEqual(batches.at(-1).ids, ['201'], '重取目标 = 打开的 activeArticleId');
  await act(async () => { batches.at(-1).d.resolve(rows2); });
  await flush(3);
  assert.equal(getBodyEntry('201')?.state, 'ready');
  await waitFor(() => entry.container.querySelector('.article-prose')?.textContent.includes('201 修订二'),
    { label: 'Reader 渲染修订二（自动恢复闭环）', timeout: 8000 });

  /* ---- 场景 2：在途（loading）期间再推进修订 → 失效；旧戳响应必须被丢弃 ---- */
  await act(async () => { useAppStore.getState().retryHydration('201'); }); // 重入队：loading + 新一轮在途请求
  await flush(2);
  assert.equal(getBodyEntry('201')?.state, 'loading');
  const pendingOld = batches.at(-1); // 旧戳在途请求（发起于失效前）
  const oldStamp = getBodyEntry('201')?.contentRevision ?? 0;

  const rows3 = [ROW(3, '<p>201 修订三 最新</p>')];
  setCurrentRows(rows3);
  await act(async () => { await useAppStore.getState().reloadFromBackend(); });
  const afterRevBump2 = getBodyEntry('201');
  assert.ok(afterRevBump2?.state === 'cleared' || afterRevBump2?.state === 'loading',
    `在途期间修订推进 → 旧响应戳过期（实际 ${afterRevBump2?.state}）`);
  assert.equal(afterRevBump2?.content, '', '旧正文不得留存');

  /* Reader 失效观察已自动重新入队（新一轮请求）——旧响应随后放行。 */
  await waitFor(() => batches.length >= 3, { label: 'Reader 失效后自动重新入队', timeout: 8000 });
  await act(async () => { pendingOld.d.resolve(rows2); }); // 旧戳（修订二）迟到
  await flush(3);
  assert.equal(getBodyEntry('201')?.state, 'loading',
    '旧戳响应被整体丢弃：记录保持新一轮 loading（不被旧正文复活为 ready）');
  assert.equal(getBodyEntry('201')?.content, '', '旧正文不得回填');

  await act(async () => { batches.at(-1).d.resolve(rows3); });
  await flush(3);
  assert.equal(getBodyEntry('201')?.state, 'ready');
  await waitFor(() => entry.container.querySelector('.article-prose')?.textContent.includes('201 修订三'),
    { label: '新响应落地渲染修订三', timeout: 8000 });
  assert.equal(getBodyEntry('201')?.contentRev, 3, '记录携带 DB 修订三身份');
  assert.ok(getArticlesCalls() >= 3, '整条链路真实走 get_articles IPC（非状态自证）');
  const oldStampStillStale = (getBodyEntry('201')?.contentRevision ?? 0) !== oldStamp;
  assert.ok(oldStampStillStale, '显式失效计数在重入队后保持推进（旧戳窗口关闭，非重置后巧合匹配）');
});
