// OPT-008B R2（主控清单②）：搜索跨布局结果的**真实组件**打开/返回/续页回归。
//
// 为什么必须挂组件：R1 的搜索回退（anchorToArticle 在 article_index=null 时按 id
// 取详情并把视图行注入 entries + 置 activeArticleId）此前只有 store 层断言
// （dedup-display (E)：get_article 调用 + entries.some + activeArticleId）——
// 「数组里有条目 + activeID 正确」不能证明真实界面呈现：Reader 是否真渲染该行、
// 当前布局的卡片是否被伪造成另一布局、列表窗口/分页游标是否被污染，都只在真实
// 挂载树 + 真实 DOM 上可判。本用例按真实消费链取证：
//   1. 挂载真实 App（含真实 SearchModal）；
//   2. 在搜索输入框真实输入 → 防抖 → mock 后端返回**另一布局（social）**的行；
//   3. 点击结果项（真实 runItem → anchorScopeNav → anchorToArticle 回退路径）；
//   4. 断言 Reader 真实渲染标题/正文（不是仅 store 布尔）；
//   5. 断言当前 article 布局的卡片形态不变（不伪造 social 卡）、可见列表不含它
//      （布局过滤仍是查询不变量）；
//   6. Escape 返回：Reader 回到空态；
//   7. 续页边界（第二例）：注入行在 entries 期间，自动/显式 loadMoreArticles 的
//      在途响应照常落地并推进游标——注入不得动游标/窗口（deepEqual 不变量）。
//
// 后端 mock 只实现数据面（list/search/article_index 的真实口径：搜索跨布局命中、
// 位置查询按布局过滤返回 null），不替身任何应用逻辑。
// 卫生：本文件的 plan 覆写（list_articles/article_index/search_articles）在用例
// 结束（含断言失败）时经 finally resetPlan() 复原，避免跨用例模块级泄漏。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, waitFor, resetStore, useAppStore, pressKey, storeMod } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, setPlan, resetPlan, clearInvokeCalls, invokeCalls, deferred } from './fixtures.mjs';

const { selectVisibleEntries } = storeMod;

/** 真实输入：绕过 React value tracker 后派发 input 事件（触发 onchange 真实路径）。 */
function typeInput(input, value) {
  const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(input), 'value')?.set;
  assert.ok(setter, '应从 InputElement.prototype 取得受控 value setter');
  setter.call(input, value);
  input.dispatchEvent(new Event('input', { bubbles: true }));
}

function layoutOf(row) {
  return Number(row.feed_id) === 11 ? 'social' : 'article';
}

function rowsForLayoutSorted(rows, layout) {
  return rows
    .filter((r) => layoutOf(r) === layout)
    .sort((x, y) => {
      const t = String(y.published_at).localeCompare(String(x.published_at));
      return t !== 0 ? t : Number(y.id) - Number(x.id);
    });
}

/** 按后端真实口径 mock article_index：目标行不在该布局的显示集合内 → null。 */
function planLayoutAwareArticleIndex(rows) {
  setPlan({
    article_index: (a) => {
      const layout = a?.args?.layout;
      const list = rowsForLayoutSorted(rows, layout);
      const idx = list.findIndex((r) => Number(r.id) === Number(a?.articleId));
      return Promise.resolve(idx >= 0 ? idx : null);
    },
  });
}

/* ============================================================
用例 1：真实搜索 UI → 点击跨布局结果 → Reader 真实渲染 → Escape 返回。
============================================================ */
componentTest('OPT-008B 搜索跨布局结果：真实搜索面板点击打开，Reader 实际渲染（非数组自证），Escape 返回且不伪造布局卡片', { timeout: 40000 }, async () => {
  try {
    installBackend();
    clearInvokeCalls();
    resetStore();
    const rows = [
      mkRow({ id: 101, title: 'A1 文章卡一', url: 'https://example.com/a1', published_at: '2026-10-01T10:00:00Z' }),
      mkRow({ id: 102, title: 'A2 文章卡二', url: 'https://example.com/a2', published_at: '2026-10-02T10:00:00Z' }),
      mkRow({
        id: 201,
        feed_id: 11,
        title: '跨布局社交卡 Z',
        snippet: '跨布局摘要',
        url: 'https://example.com/z',
        image_url: 'https://img.example/z.png',
        content_html: '<p>跨布局正文段落</p>',
        body_text: '跨布局正文段落',
        published_at: '2026-10-03T10:00:00Z',
      }),
    ];
    seedFromRows(rows, { layout: 'article' });
    planLayoutAwareArticleIndex(rows);
    setPlan({ search_articles: () => Promise.resolve(rows.filter((r) => r.id === 201)) });

    const { createElement, act } = await import('react');
    const { default: App } = await import('../../src/App.tsx');
    const entry = await mount(createElement(App));

    await waitFor(() => entry.container.querySelectorAll('.article-card').length === 2,
      { label: 'App 启动后 article 卡片出现', timeout: 8000 });
    assert.equal(entry.container.querySelectorAll('.social-card').length, 0,
      'article 布局不应渲染 social 卡片（打开前基线）');
    const visibleIdsBefore = selectVisibleEntries(useAppStore.getState()).map((e) => e.id);
    assert.deepEqual(visibleIdsBefore, ['102', '101'], '打开前可见列表 = 当前布局行（newest-first）');

    // ---- 真实搜索面板：输入 → 防抖 → 后端返回跨布局行 ----
    await act(async () => { useAppStore.getState().openSearch(); });
    await flush(2);
    const input = entry.container.querySelector('.search-modal-input');
    assert.ok(input, '搜索面板输入框应真实出现');
    await act(async () => { typeInput(input, '跨布局'); });
    await waitFor(
      () => [...entry.container.querySelectorAll('.cp-item')].some((el) => el.textContent.includes('跨布局社交卡 Z')),
      { label: '搜索结果出现跨布局行', timeout: 8000 },
    );
    const resultItem = [...entry.container.querySelectorAll('.cp-item')]
      .find((el) => el.textContent.includes('跨布局社交卡 Z'));
    assert.ok(invokeCalls.filter((c) => c.cmd === 'search_articles').length >= 1,
      '搜索必须走真实 search_articles IPC');

    // ---- 点击结果项（真实 runItem → anchorToArticle 回退路径）----
    await act(async () => { resultItem.click(); });
    await waitFor(() => useAppStore.getState().activeArticleId === '201',
      { label: '点击后 activeArticleId 落地为跨布局行', timeout: 8000 });

    // 真实 DOM 取证：Reader 实际渲染该行标题与正文（不是「数组存在 + activeID」自证）
    await waitFor(
      () => entry.container.querySelector('.reader-article-title')?.textContent === '跨布局社交卡 Z',
      { label: 'Reader 实际渲染跨布局标题', timeout: 8000 },
    );
    assert.ok(entry.container.querySelector('.reader-active-view'), 'Reader 应处于激活视图（非空态）');
    await waitFor(() => entry.container.querySelector('.article-prose')?.textContent.includes('跨布局正文段落'),
      { label: 'Reader 正文水合完成（真实 get_article 消费）', timeout: 8000 });

    // 查询不变量：当前布局的卡片形态不因注入而伪造（social 卡不得出现在 article 布局），
    // 可见列表仍按布局过滤（条目在 entries 里可用于打开，但不进入 article 的列表集合）。
    assert.equal(entry.container.querySelectorAll('.social-card').length, 0,
      'article 布局不得因跨布局注入而渲染 social 卡片');
    assert.equal(entry.container.querySelectorAll('.article-card').length, 2,
      'article 卡片数不应被注入行改变');
    const st = useAppStore.getState();
    assert.ok(st.entries.some((e) => e.id === '201'), '注入行在 entries 中（供 Reader 消费）');
    assert.deepEqual(selectVisibleEntries(st).map((e) => e.id), visibleIdsBefore,
      '可见列表（布局过滤后）不受注入影响');

    // ---- Escape 返回：Reader 回空态，注入行仍在 entries（列表不受影响）----
    await act(async () => { pressKey('Escape'); });
    await flush(2);
    assert.equal(useAppStore.getState().activeArticleId, null, 'Escape 应关闭阅读器选中');
    assert.ok(entry.container.querySelector('.reader-empty-state'), 'Reader 应回到空态（真实 DOM）');
    assert.equal(entry.container.querySelectorAll('.article-card').length, 2,
      '返回后列表卡片保持不变');
  } finally {
    resetPlan();
  }
});

/* ============================================================
用例 2：跨布局注入期间，自动续页的在途响应照常落地并推进游标；
注入行不污染游标/可见集合（deepEqual 精确不变量）。
============================================================ */
componentTest('OPT-008B 跨布局注入与在途续页：注入行动游标为零；首次响应被代际丢弃、自动重试落地推进（不污染查询窗口）', { timeout: 40000 }, async () => {
  try {
    installBackend();
    clearInvokeCalls();
    resetStore();

    // 501 条 article 行（101..601）：首屏 500 条（601..102），续页恰 1 条（101）。
    const base = Date.UTC(2026, 0, 1);
    const rows = [];
    for (let id = 101; id <= 601; id++) {
      rows.push(mkRow({
        id,
        title: `A${id}`,
        url: `https://example.com/a${id}`,
        published_at: new Date(base + id * 60000).toISOString(),
      }));
    }
    rows.push(mkRow({
      id: 9001, feed_id: 11, title: '跨布局社交卡 Z', url: 'https://example.com/z',
      image_url: 'https://img.example/z.png',
      content_html: '<p>跨布局正文段落</p>',
      body_text: '跨布局正文段落',
      published_at: new Date(base + 999999 * 60000).toISOString(),
    }));

    /* 忠实分页 mock：首屏按 offset/limit；keyset 续页**两次请求各一个受控
       deferred**——首次响应在锚定推进查询代次后到达（被代际守卫丢弃），
       articlesLoading 复位触发自动重试（第二次请求）落地推进。区分两者是
       本用例的取证核心（不声称「原响应照常落地」）。
       （jsdom 无布局：Timeline 的 filledViewport 恒 false，装载后会真实触发
       自动续拉——这正是生产路径的再一档消费。） */
    const firstPage2 = deferred();
    const retryPage2 = deferred();
    let keysetCalls = 0;
    setPlan({
      list_articles: (a) => {
        const la = a?.args ?? {};
        if (la.last_id != null && la.last_published != null) {
          keysetCalls += 1;
          return keysetCalls === 1 ? firstPage2.promise : retryPage2.promise;
        }
        const list = rowsForLayoutSorted(rows, la.layout);
        const from = typeof la.offset === 'number' ? la.offset : 0;
        return Promise.resolve(list.slice(from, from + (la.limit ?? list.length)));
      },
    });
    planLayoutAwareArticleIndex(rows);

    seedFromRows(rows, { layout: 'article' });
    const { createElement, act } = await import('react');
    const { default: App } = await import('../../src/App.tsx');
    const entry = await mount(createElement(App));

    // 首屏 500 行装载后，自动续页（keyset 锚 = 102）已在途（**首次**请求挂起）。
    await waitFor(
      () => useAppStore.getState().entries.length === 500 && useAppStore.getState().articlesLoading === true,
      { label: '首屏 500 行装载 + 自动续页在途', timeout: 10000 },
    );
    const cursorBeforeOpen = useAppStore.getState().articlesCursor['article|all'];
    assert.deepEqual(
      { lastId: cursorBeforeOpen?.lastId, loaded: cursorBeforeOpen?.loaded },
      { lastId: 102, loaded: 500 },
      '首屏游标应为第 500 行（id=102）',
    );

    // ---- 续页在途期间：跨布局打开（与搜索结果项同一 action）注入行 ----
    await act(async () => { await useAppStore.getState().anchorToArticle('9001'); });
    await flush(3);
    let st = useAppStore.getState();
    assert.equal(st.activeArticleId, '9001', '跨布局行应被打开');
    assert.ok(st.entries.some((e) => e.id === '9001'), '详情行注入 entries（Reader 可渲染）');
    assert.deepEqual(st.articlesCursor['article|all'], cursorBeforeOpen,
      '回退注入不得改动分页游标（不污染查询窗口）');
    assert.equal(st.entries.length, 501, 'entries = 首屏 500 + 注入 1（不整体替换窗口）');
    assert.ok(!selectVisibleEntries(st).some((e) => e.id === '9001'),
      '注入行不进入当前布局的可见集合（布局过滤不变量）');
    assert.equal(entry.container.querySelector('.reader-article-title')?.textContent, '跨布局社交卡 Z',
      'Reader 在真实挂载树上渲染注入行（同用例 1 的 DOM 取证）');
    assert.equal(keysetCalls, 1, '首次自动续页必须携带 keyset 锚（真实分页形态）');

    /* ---- 放行**首次**续页响应：锚定推进了该键首屏代次 → 响应属于旧查询实例，
       被代际守卫丢弃（不动游标、不接入行）。机制依据：anchorToArticle 入口
       beginGlobalQuery；落地判据 currentFirstPageSerial(queryKey) !== firstPageGen。 */
    const page2Rows = rowsForLayoutSorted(rows, 'article').slice(500); // [id=101]
    assert.equal(page2Rows.length, 1, '续页恰 1 行（前置）');
    await act(async () => { firstPage2.resolve(page2Rows); });
    await flush(3);
    st = useAppStore.getState();
    assert.deepEqual(st.articlesCursor['article|all'], cursorBeforeOpen,
      '首次响应被代际丢弃：游标保持原锚（不动）');
    assert.ok(!st.entries.some((e) => e.id === '101'),
      '首次响应被代际丢弃：续页行未被接入（不是「原响应照常落地」）');

    // ---- 丢弃后 articlesLoading 复位 → Timeline 自动重试（第二次 keyset 请求）→ 落地。
    await waitFor(() => keysetCalls === 2, { label: '自动重试续页发起（第二次 keyset 请求）', timeout: 8000 });
    await act(async () => { retryPage2.resolve(page2Rows); });
    await flush(4);
    st = useAppStore.getState();
    const cursorAfter = st.articlesCursor['article|all'];
    assert.deepEqual(
      { lastId: cursorAfter?.lastId, loaded: cursorAfter?.loaded },
      { lastId: 101, loaded: 501 },
      '自动重试的续页响应落地并推进游标到新页末行（id=101, loaded=501）——注入行未被当作锚',
    );
    assert.equal(cursorAfter?.lastPublished, rows.find((r) => r.id === 101).published_at,
      'keyset 锚应为续页真实末行的 published_at 原文');
    assert.ok(st.entries.some((e) => e.id === '101'), '续页行真实追加进 entries');
    assert.equal(st.entries.length, 502, 'entries = 500 + 注入 1 + 续页 1');
    const visible = selectVisibleEntries(st).map((e) => e.id);
    assert.equal(visible.length, 501, '可见集合 = 全部 article 行（含续页）');
    assert.equal(visible[visible.length - 1], '101', '续页行在可见集合末尾（时间序正确）');
    assert.ok(!visible.includes('9001'), '跨布局注入行始终不进入 article 可见集合');
  } finally {
    resetPlan();
  }
});
