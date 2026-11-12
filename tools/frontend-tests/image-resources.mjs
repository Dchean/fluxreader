// Shared with ui-contracts; this isolated domain does not need the unfinished localSync Rust contract.
export const id = 'image-resources';
export async function run(ctx) {
  const { store, checkNew } = ctx;
/* Media resource contracts supersede dataURL/URL-only permanent-failure assertions.
 * Real effects, cancellation, budgets, decode locality, player and Reader ownership are exercised
 * by component-tests/image-resources.test.mjs and gallery-media.test.mjs, not source-shape mirrors. */
{
  const media = await import('../../src/lib/imageProxy.ts');
  const { imageBytes } = await import('../../dist-test/lib/api.js');
  checkNew('(media-route) 代理白名单仍唯一，普通域/data URL不新增代理',
    media.needsImageProxy('https://cdnfile.sspai.com/a.png')
    && media.needsImageProxy('https://img.doubanio.com/a.jpg')
    && !media.needsImageProxy('https://example.com/a.png') && !media.needsImageProxy('data:image/png;base64,AA'));
  checkNew('(media-key) 同URL不同有效Referer隔离，不删除图片签名查询',
    media.imageResourceKey('https://cdnfile.sspai.com/a.png?t=1','https://sspai.com/a')
      !== media.imageResourceKey('https://cdnfile.sspai.com/a.png?t=1','https://sspai.com/b')
    && media.imageResourceKey('https://cdnfile.sspai.com/a.png?t=1') !== media.imageResourceKey('https://cdnfile.sspai.com/a.png?t=2'));
  checkNew('(media-ipc) raw ArrayBuffer和number[] fallback实际校验并归一',
    imageBytes(new Uint8Array([137,80,78,71]).buffer)[0] === 137 && imageBytes([0,255])[1] === 255);
  let invalidRejected = false;
  try { imageBytes([256]); } catch { invalidRejected = true; }
  checkNew('(media-ipc) 非byte响应不能默默截断为合法图片', invalidRejected);
  const { renderToStaticMarkup } = await import('react-dom/server');
  const { createElement } = await import('react');
  const { CoverImage } = await import('../../src/components/CoverImage.tsx');
  const render = src => renderToStaticMarkup(createElement(CoverImage,{src,className:'card-cover-thumb',alt:'cover'}));
  checkNew('(cov-empty) 空封面保持无img',render('') === '' && !render(null).includes('<img'));
  checkNew('(cov-proxy) 代理未获得租约前保持同尺寸占位，不旁路直连',
    !render('https://cdnfile.sspai.com/a.png').includes('<img') && render('https://cdnfile.sspai.com/a.png').includes('card-cover-thumb cover-fallback'));
  checkNew('(cov-direct) 普通图片保留no-referrer直连',
    render('https://plain.example/a.png').includes('https://plain.example/a.png') && /referrerpolicy="no-referrer"/i.test(render('https://plain.example/a.png')));
  store.getState().playPodcastEpisode('A','show','https://plain.example/cover.png','https://plain.example/a.mp3','21');
  store.getState().playPodcastEpisode('B','show','','https://plain.example/b.mp3','22');
  checkNew('(cov-owner) 无新封面时cover及所属条目一起沿用',store.getState().player.coverEntryId === '21' && store.getState().player.cover === 'https://plain.example/cover.png');
  store.getState().closePodcastBar();
  store.getState().openLightbox('https://plain.example/body.png',undefined,'https://plain.example/article');
  checkNew('(lightbox-source) 正文图保持原URL/pageUrl且无封面写owner',store.getState().lightboxEntryId === null && store.getState().lightboxPageUrl === 'https://plain.example/article');
  store.getState().closeLightbox();
  checkNew('(lightbox-close) 关闭清空来源',store.getState().lightboxUrl === null && store.getState().lightboxPageUrl === undefined);
}

}

