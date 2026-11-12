// 阅读动作必须区分分区：文章选择展示正文，其它选择只移动当前项。
// 自动标读由打开设置控制；显式标读仍可执行。生产 Timeline/Reader slice + fake IPC。
import assert from 'node:assert/strict';
import { componentTest, mount, flush, resetStore, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, clearInvokeCalls, invokeCalls, resetPlan } from './fixtures.mjs';

for (const [layout, feedId] of [['article', 10], ['social', 11], ['image', 12], ['podcast', 13], ['notification', 14]]) {
  for (const markReadOnOpen of [false, true]) {
    componentTest(`阅读动作 ${layout} / 打开标读${markReadOnOpen ? '开' : '关'}：选择、打开与自动副作用各归其位`, { timeout: 20000 }, async () => {
      installBackend(); resetPlan(); resetStore();
      seedFromRows([mkRow({ id: 801, feed_id: feedId, image_url: 'https://image.example/a.png', enclosure_url: 'https://audio.example/a.mp3', content_html: '<p>正文</p>' })], { layout });
      useAppStore.setState((s) => ({ settings: { ...s.settings, markReadOnOpen } }));
      const { createElement, act } = await import('react');
      const { Timeline } = await import('../../src/components/Timeline.tsx');
      const entry = await mount(createElement(Timeline));
      await flush(2);
      clearInvokeCalls();
      await act(async () => { useAppStore.getState().selectArticle('801'); });
      await flush(2);
      const state = () => useAppStore.getState();
      const read = () => state().entries.find((item) => item.id === '801').isRead;
      assert.equal(state().activeArticleId, '801', '五分区都能选择当前项');
      assert.equal(read(), layout === 'article' && markReadOnOpen, '只有实际展示正文的文章选择按设置标读');
      assert.equal(invokeCalls.filter((c) => c.cmd === 'get_article').length, layout === 'article' ? 1 : 0,
        '非文章选中不走 Reader 详情/全文路径');
      assert.equal(invokeCalls.filter((c) => ['ai_summarize', 'ai_translate', 'extract_fulltext'].includes(c.cmd)).length, 0,
        '仅选择不借隐藏 Reader 发起付费或全文请求');
      if (layout === 'image' || layout === 'podcast') {
        const trigger = entry.container.querySelector(layout === 'image' ? '.gallery-card img' : '.podcast-card');
        assert.ok(trigger, '真实卡片动作入口存在');
        await act(async () => { trigger.click(); });
        await flush(2);
        assert.equal(read(), markReadOnOpen, '打开图片/开始播放也服从同一设置');
        if (layout === 'image') {
          assert.ok(state().lightboxUrl, '图片真正打开');
          assert.equal(state().lightboxEntryId, '801', '灯箱失败上报保留真实条目身份');
        }
        else assert.equal(state().player.isPlaying, true, '播客进入播放态');
      }
      if (!markReadOnOpen) {
        await act(async () => { state().toggleEntryFlag('801', 'isRead'); });
        await flush(2);
        assert.equal(read(), true, '关闭自动标读不阻止用户显式标读');
      }
    });
  }
}
