// OPT-008B R3（主控清单⑥）：五布局 × 共享内容修订变化的真实组件回归。
//
// 取证目标：五个生效布局（article/social/image/podcast/notification）各自的
// 条目引用**同一 contents 行**（同 content_id，模拟跨源/跨布局共享正文）——
// 该内容 revision 推进后，每个布局的条目在真实 reload 里都失效清空正文，并
// 经真实重水合链路拿到新修订；Reader 对每个布局的文章给出实际渲染证据
// （Reader 入口不依赖卡片是否挂载）。social 布局额外取证卡片 DOM（.social-text）
// 与「同一窗口两个引用条目都失效」；不使用扩大无关全量审计替代。
//
// 时序控制：假后端的 get_articles 在观察窗口内挂起（deferred），使「失效清空」
// 在没有重取完成的确定时点上可观测；观察后放行 → 断言新修订落地。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, getBodyEntry, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, setCurrentRows, setPlan, deferred, clearInvokeCalls } from './fixtures.mjs';

const ORDER = ['article', 'social', 'image', 'podcast', 'notification'];
const CARD = { article: '.article-card', social: '.social-card', image: '.gallery-card', podcast: '.podcast-card', notification: '.notif-card' };
const FEED = { article: 10, social: 11, image: 12, podcast: 13, notification: 14 };
const PRIMARY_ID = { article: 601, social: 602, image: 603, podcast: 604, notification: 605 };
/** social 布局的第二个引用条目（同 feed、同 content_id——同一窗口内的共享兄弟） */
const SOCIAL_SIBLING_ID = 606;
const SHARED_CONTENT_ID = 7000;

const marker = (rev, layout) => `共享内容 v${rev} ${layout}`;
const rowFor = (layout, rev) => mkRow({
  id: PRIMARY_ID[layout],
  feed_id: FEED[layout],
  title: `${layout} 共享文章`,
  snippet: `snippet ${layout}`,
  url: `https://example.com/${layout}`,
  content_id: SHARED_CONTENT_ID,
  content_revision: rev,
  content_html: `<p>${marker(rev, layout)}</p>`,
});
const siblingRow = (rev) => mkRow({
  id: SOCIAL_SIBLING_ID,
  feed_id: FEED.social,
  title: 'social 共享文章（另一条目）',
  snippet: 'snippet social sibling',
  url: 'https://example.com/social-sibling',
  content_id: SHARED_CONTENT_ID,
  content_revision: rev,
  content_html: `<p>${marker(rev, 'social-sibling')}</p>`,
});

componentTest('OPT-008B R3 五布局共享正文修订：revision 推进后各布局条目失效、真实重水合与 Reader 渲染证据', { timeout: 60000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();

  let revision = 1;
  const rowsFor = (rev) => [...ORDER.map((l) => rowFor(l, rev)), siblingRow(rev)];
  let rowsCurrent = rowsFor(revision);
  seedFromRows(rowsCurrent, { layout: 'article' });
  setCurrentRows(rowsCurrent);

  /* 受控 get_articles：hold=true 时挂起（观察失效窗口），否则按当前行立即返回。 */
  let hold = false;
  const held = [];
  setPlan({
    get_articles: (args) => {
      const ids = (args?.ids ?? []).map(String);
      if (hold) {
        const d = deferred();
        held.push({ ids, d });
        return d.promise;
      }
      return Promise.resolve(rowsCurrent.filter((r) => ids.includes(String(r.id))));
    },
  });

  const { createElement, act } = await import('react');
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const { Reader } = await import('../../src/components/Reader.tsx');
  const entry = await mount(createElement('div', null, createElement(Timeline), createElement(Reader)));
  await flush(3);

  const proseText = () => entry.container.querySelector('.article-prose')?.textContent ?? '';
  /** 放行全部挂起请求（按当前行集返回），随后由真实链路落地。 */
  const releaseHeld = async () => {
    hold = false;
    await act(async () => {
      for (const h of held.splice(0)) h.d.resolve(rowsCurrent.filter((r) => h.ids.includes(String(r.id))));
      await Promise.resolve();
    });
    await flush(3);
  };

  for (const layout of ORDER) {
    /* 1) 切到该布局：真实 list_articles（后端布局过滤口径）替换窗口 */
    await act(async () => { useAppStore.getState().selectLayout(layout); });
    await flush(3);
    const id = String(PRIMARY_ID[layout]);
    assert.ok(entry.container.querySelector(`${CARD[layout]}`), `[${layout}] 基线卡片已挂载`);

    /* 2) 打开 + 水合该布局的共享条目（真实批量水合 IPC） */
    await act(async () => { useAppStore.setState({ activeArticleId: id }); });
    const targetIds = layout === 'social' ? [id, String(SOCIAL_SIBLING_ID)] : [id];
    await act(async () => { useAppStore.getState().hydrateArticleContent(targetIds); });
    await waitFor(
      () => targetIds.every((t) => getBodyEntry(t)?.state === 'ready'),
      { label: `[${layout}] 初始水合 ready`, timeout: 8000 },
    );
    for (const t of targetIds) {
      assert.equal(getBodyEntry(t)?.contentId, SHARED_CONTENT_ID, `[${layout}] ${t} 引用同一共享 contents 行`);
      assert.equal(getBodyEntry(t)?.contentRev, revision, `[${layout}] ${t} 记录当前修订`);
    }
    await waitFor(() => proseText().includes(marker(revision, layout)), { label: `[${layout}] Reader 渲染修订 v${revision}`, timeout: 8000 });

    /* 3) 共享内容修订推进（同一 contents 行 → 全部引用行的 revision 同步前进） */
    revision += 1;
    rowsCurrent = rowsFor(revision);
    setCurrentRows(rowsCurrent);
    hold = true; // 观察窗口：重取挂起，失效状态确定可测
    await act(async () => { await useAppStore.getState().reloadFromBackend(); });

    /* 4) 失效证据：该布局条目（social 含兄弟条目）在真实 reload 后不再 ready、
       旧正文清空；五布局行仍共享同一 content_id（同一次 revision 推进送达全部） */
    for (const t of targetIds) {
      const rec = getBodyEntry(t);
      assert.ok(rec && rec.state !== 'ready' && rec.content === '',
        `[${layout}] ${t} 在共享内容修订推进后失效清空（实际 ${rec?.state}/${rec?.content}）`);
    }
    assert.ok(rowsCurrent.every((r) => r.content_id === SHARED_CONTENT_ID && r.content_revision === revision),
      `[${layout}] 五布局行指向同一 contents 行的同一新修订`);

    /* 5) 真实重水合落地：记录带新修订身份，Reader 实际渲染新内容 */
    await waitFor(() => {
      const covered = new Set(held.flatMap((h) => h.ids));
      return targetIds.every((t) => covered.has(t));
    }, { label: `[${layout}] 失效后重新入队（Reader/卡片，覆盖全部目标条目）`, timeout: 8000 });
    await releaseHeld();
    await waitFor(
      () => targetIds.every((t) => getBodyEntry(t)?.state === 'ready' && getBodyEntry(t)?.contentRev === revision),
      { label: `[${layout}] 新修订落地记录`, timeout: 8000 },
    );
    await waitFor(() => proseText().includes(marker(revision, layout)), { label: `[${layout}] Reader 渲染新修订 v${revision}`, timeout: 8000 });

    /* 6) social 布局额外取证：兄弟条目记录也恢复新修订；卡片 DOM 实际显示新正文 */
    if (layout === 'social') {
      await waitFor(
        () => getBodyEntry(String(SOCIAL_SIBLING_ID))?.state === 'ready'
          && getBodyEntry(String(SOCIAL_SIBLING_ID))?.contentRev === revision,
        { label: '[social] 兄弟条目记录恢复新修订', timeout: 8000 },
      );
      await waitFor(
        () => entry.container.querySelector('.social-text')?.textContent.includes(marker(revision, 'social')),
        { label: '[social] 卡片 DOM 显示新修订正文（reference 重取闭环）', timeout: 8000 },
      );
    }
  }

  /* 收口：五个布局各推进过一次共享修订（v1→v6），全程无卡片/Reader 退化 */
  assert.equal(revision, 6, '五布局各推进一次修订');
  assert.ok(entry.container.querySelector('.notif-card'), '末轮布局卡片仍在（不因连续失效/重取崩溃）');
});
