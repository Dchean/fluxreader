// 审计卡2-2：Reader 正文三态（真实 Reader 挂载）——loading/failed/missing
// 不再直传 ReaderProse 空渲染，与 SocialCard 同构（对照组 Timeline.tsx 卡片
// 分支、t114-x1b 回归）：
//   loading/未请求 → .hydrate-placeholder「加载正文…」（且无 .article-prose）；
//   failed → .hydrate-retry 内联重试（点击真实重入队，命中后回 .article-prose）；
//   missing → 缺行文案（.hydrate-placeholder）。
//
// 链路口径：打开（selectArticle）走**单篇详情** get_article；失败重试
// （retryHydration）走**批量** get_articles——两条 IPC 都 deferred 手动放行，
// 分别驱动「在途窗口 / 失败 / 命中 / 缺行」。断言同时读 DOM 与 dist-test
// bodyCache 记录（组件与断言同一份实例）。
// 断言卫生：一律 assert.ok(布尔)——不把 DOM 元素直接交给 assert.equal（失败时
// Node 的 assertion diff 会对 jsdom 元素做 Myers diff，实测耗尽 40s 量级）。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, getBodyEntry, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, setPlan, resetPlan, deferred } from './fixtures.mjs';

const ROWS = [
  mkRow({ id: 201, title: '文章 201', snippet: 's201' }),
  mkRow({ id: 202, title: '文章 202', snippet: 's202' }),
];
const contentRow = (row, html) => ({ ...row, content_html: html });

componentTest('Reader 正文三态：loading 占位 / failed 内联重试 / missing 文案（与卡片同构）', { timeout: 40000 }, async () => {
  installBackend();
  resetStore();
  seedFromRows(ROWS, { layout: 'article' });

  const details = [];   // get_article（打开 = 单篇详情）挂起项
  const batches = [];   // get_articles（重试 = 批量）挂起项
  try {
    setPlan({
      get_article: (a) => {
        const d = deferred();
        details.push({ id: String(a.id), d });
        return d.promise;
      },
      get_articles: (a) => {
        const d = deferred();
        batches.push({ ids: a.ids.map(String), d });
        return d.promise;
      },
    });

    const { createElement, act } = await import('react');
    const { Reader } = await import('../../src/components/Reader.tsx');
    const entry = await mount(createElement(Reader));

    // ---- loading/未请求：打开即在途占位，不渲染 .article-prose ----
    await act(async () => { useAppStore.getState().selectArticle('201'); });
    await waitFor(() => getBodyEntry('201')?.state === 'loading', { label: '201 进入 loading 记录' });
    await flush(2);
    assert.ok(entry.container.querySelector('.reader-active-view .hydrate-placeholder')?.textContent.includes('加载正文…'),
      '在途时正文区应显示加载占位（与卡片同构）');
    assert.ok(!entry.container.querySelector('.reader-active-view .article-prose'),
      '在途时不得渲染空的 ReaderProse 正文块');

    // ---- failed：详情响应失败落 failed 终态，内联重试入口文案取记录 message ----
    await act(async () => {
      details.at(-1).d.reject(new Error('注入失败：db busy'));
      await Promise.resolve();
    });
    await flush(3);
    assert.equal(getBodyEntry('201')?.state, 'failed', '失败应落 failed 记录');
    const failure = entry.container.querySelector('.reader-active-view .hydrate-failure');
    const retry = failure?.querySelector('.hydrate-retry');
    assert.ok(failure?.textContent.includes('正文暂时无法加载'), '失败时应显示失败文案');
    assert.equal(retry?.textContent.trim(), '重试', '失败时应显示独立重试按钮');
    assert.equal(
      failure.querySelector('.hydrate-failure-text')?.getAttribute('title'),
      getBodyEntry('201').message,
      '错误详情取自同一记录 message',
    );

    // ---- 重试命中：retryHydration 真实重入队（批量）→ ready → 回到正文渲染 ----
    await act(async () => { retry.click(); });
    await flush(2);
    assert.deepEqual(batches.at(-1).ids, ['201'], '重试只请求该文章 id');
    await act(async () => {
      batches.at(-1).d.resolve([contentRow(ROWS.find((r) => r.id === 201), '<p>201 正文已到达</p>')]);
      await Promise.resolve();
    });
    await waitFor(() => getBodyEntry('201')?.state === 'ready', { label: '201 重试命中 ready' });
    await flush(2);
    assert.ok(entry.container.querySelector('.reader-active-view .article-prose')?.textContent.includes('201 正文已到达'),
      '命中后应回到 ReaderProse 渲染真实正文');
    assert.ok(!entry.container.querySelector('.reader-active-view .hydrate-retry'), '重试入口应消失');

    // ---- missing：重试后批量响应缺行 → 缺行终态文案（无重试入口）----
    await act(async () => { useAppStore.getState().selectArticle('202'); });
    await waitFor(() => getBodyEntry('202')?.state === 'loading', { label: '202 进入 loading 记录' });
    await act(async () => {
      details.at(-1).d.reject(new Error('注入失败：db busy'));
      await Promise.resolve();
    });
    await flush(3);
    await act(async () => { entry.container.querySelector('.reader-active-view .hydrate-retry').click(); });
    await flush(2);
    await act(async () => {
      batches.at(-1).d.resolve([]); // 空 rows = 该文章不存在
      await Promise.resolve();
    });
    await flush(3);
    assert.equal(getBodyEntry('202')?.state, 'missing', '缺行应落 missing 终态');
    assert.ok(entry.container.querySelector('.reader-active-view .hydrate-placeholder')?.textContent.includes('文章不存在'),
      '缺行应呈现「文章不存在」而非永挂加载');
    assert.ok(!entry.container.querySelector('.reader-active-view .hydrate-retry'), 'missing 终态无重试入口');
  } finally {
    /* 卫生：plan 覆写（含 deferred 挂起器）必须在用例结束（含断言失败）时复原，
       否则残留的 get_article/get_articles defer 会拦截后续用例的详情 IPC。 */
    resetPlan();
  }
});
