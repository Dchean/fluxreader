// The shared service is tested through real public acquire/release and fake IPC, not a second queue.
import assert from 'node:assert/strict';
import { componentTest as harnessTest, mount, unmountAll, flush, waitFor, resetStore, useAppStore, dom } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, resetPlan, setPlan, deferred, invokeCount, invokeCalls } from './fixtures.mjs';
const { createElement, act } = await import('react');
const media = await import('../../src/lib/imageProxy.ts');
const cover = await import('../../src/lib/coverImage.ts');
const { imageBytes } = await import('../../dist-test/lib/api.js');
const { CoverImage } = await import('../../src/components/CoverImage.tsx');
const png = (n=8192) => { const b=new Uint8Array(n); b.set([137,80,78,71]); return b.buffer; };
const image = n => `https://cdnfile.sspai.com/budget-${n}.png`;
const page = 'https://sspai.com/post/a';
const testLeases = [];
const testPending = [];
function acquire(...args) { const lease = media.acquireImage(...args); testLeases.push(lease); return lease; }
function componentTest(name, body) {
  return harnessTest(name, { timeout: 45000 }, async () => {
    try { await body(); }
    finally {
      await unmountAll();
      await act(async () => { for (const lease of testLeases.splice(0)) lease.release(); });
      for (const pending of testPending.splice(0)) pending.reject({ code: 'imageCancelled', message: 'fixture cleanup' });
      await flush(4);
      await cover.awaitCoverReportFrontier();
      media.resumeImages();
    }
  });
}
function settledImage(lease) {
  const done = () => ['ready','failed'].includes(lease.snapshot().status);
  if (done()) return Promise.resolve();
  return new Promise(resolve => {
    const off = lease.subscribe(() => { if (done()) { off(); resolve(); } });
    if (done()) { off(); resolve(); }
  });
}
function setup() { installBackend(); resetPlan(); resetStore(); cover.resetCoverCacheForTest(); }
function slowBackend({ lateRegistration=false }={}) {
  const pending=new Map();
  setPlan({fetch_image:args=>{const gate=deferred(); testPending.push(gate); pending.set(args.requestId,{...gate,args}); if(!lateRegistration)args.started.onmessage({registered:true}); return gate.promise;},
    cancel_image:({requestId})=>{pending.get(requestId)?.reject({code:'imageCancelled',message:'cancelled'}); return null;}});
  return pending;
}
componentTest('图片资源：raw/fallback字节验证，同URL不同Referer与共享lease不串失败', async()=>{
  setup();
  assert.deepEqual([...imageBytes([0,255])],[0,255]); assert.equal(imageBytes(png()).byteLength,8192);
  assert.throws(()=>imageBytes([0,-1])); assert.throws(()=>imageBytes([256])); assert.throws(()=>imageBytes([1.1])); assert.throws(()=>imageBytes({bytes:[1]}));
  setPlan({fetch_image:({pageUrl,started})=>{started.onmessage({registered:true}); if(pageUrl.endsWith('/a')) throw {code:'network',message:'offline'}; return png();}});
  const a=acquire(image('page'),page); const b=acquire(image('page'),'https://sspai.com/post/b');
  const twin=acquire(image('page'),'https://sspai.com/post/b');
  try {
    await flush(4);
    assert.equal(a.snapshot().reason,'network'); assert.equal(b.snapshot().status,'ready');
    assert.equal(twin.snapshot().src,b.snapshot().src); assert.equal(invokeCount('fetch_image'),2);
    b.failDecode(); assert.equal(twin.snapshot().status,'ready','local decode error cannot poison another consumer');
    assert.equal(cover.reportCoverFailure(a,'91',()=>true),false); assert.equal(cover.reportCoverFailure(b,'91',()=>true),false);
    assert.equal(invokeCount('report_broken_cover'),0);
  } finally {a.release();b.release();twin.release();}
});
componentTest('图片资源：登记前释放、共享消费者和旧cancel不取消新请求', async()=>{
  setup(); const pending=slowBackend({lateRegistration:true});
  const a=acquire(image('cancel'),page); const b=acquire(image('cancel'),page);
  await flush(3); const old=[...pending.values()][0];
  a.release();assert.equal(invokeCount('cancel_image'),0);
  b.release();assert.equal(invokeCount('cancel_image'),0,'not registered yet');
  await act(async()=>old.args.started.onmessage({registered:true})); await flush(3);
  assert.equal(invokeCount('cancel_image'),1); assert.equal(media.imageResourceStats().active,0);
  const next=acquire(image('cancel'),page); await flush(2);
  const fresh=[...pending.values()].at(-1); assert.notEqual(fresh.args.requestId,old.args.requestId);
  await act(async()=>{fresh.args.started.onmessage({registered:true});fresh.resolve(png());}); await flush(2);
  assert.equal(next.snapshot().status,'ready'); next.release();
});
componentTest('图片资源：96消费者限定4active/32pending，卸载不留下取图或等待队列', async()=>{
  setup(); slowBackend();
  const leases=Array.from({length:96},(_,i)=>acquire(image(`many-${i}`),page));
  try {await flush(3);const stats=media.imageResourceStats(); assert.equal(stats.active,4);assert.equal(stats.pending,32);
    assert.equal(stats.payloadBytes,100*1024*1024); assert.equal(invokeCount('fetch_image'),4);
  } finally {for(const lease of leases)lease.release();await flush(4);}
  assert.equal(invokeCount('cancel_image'),4); assert.equal(media.imageResourceStats().active,0);assert.equal(media.imageResourceStats().pending,0);
  assert.equal(media.imageResourceStats().leases,0);
});
componentTest('图片资源：128MiB预留满额保护在用Blob，释放后延期图片继续加载', async()=>{
  setup();setPlan({fetch_image:({started})=>{started.onmessage({registered:true});return png(25*1024*1024);}});
  const leases=[];const revoke=URL.revokeObjectURL;const revoked=[];
  URL.revokeObjectURL=url=>{revoked.push(url);revoke(url);};
  try {
    for(let i=0;i<5;i++){leases.push(acquire(image(`big-${i}`),page));await flush(3);}
    assert.equal(media.imageResourceStats().blobBytes,125*1024*1024);
    const protectedUrls=leases.slice(1).map(l=>l.snapshot().src);
    const waiting=acquire(image('big-next'),page);leases.push(waiting);await flush(2);
    assert.notEqual(waiting.snapshot().status,'ready');assert.equal(invokeCount('fetch_image'),5);assert.equal(revoked.length,0);
    leases[0].release();await flush(4);
    assert.equal(waiting.snapshot().status,'ready'); assert.ok(protectedUrls.every(url=>!revoked.includes(url)));
    assert.ok(media.imageResourceStats().payloadBytes<=media.IMAGE_LIMITS.payload);
  } finally {for(const l of leases)l.release();URL.revokeObjectURL=revoke;}
  assert.ok(media.imageResourceStats().idleBytes<=64*1024*1024);
});
componentTest('图片资源：lease和失败元数据均有界，显式重试不永久记住断网', async()=>{
  setup();setPlan({fetch_image:()=>png()});
  // M1: the 256 limit belongs to proxy leases; direct URLs were an invalid saturation model.
  const leases=Array.from({length:257},()=>acquire(image('lease-cap'),page));
  assert.equal(media.imageResourceStats().leases,256);assert.equal(leases[256].snapshot().status,'deferred');
  await settledImage(leases[0]);assert.equal(invokeCount('fetch_image'),1);
  await act(async()=>{for(const l of leases)l.release();});
  setPlan({fetch_image:()=>Promise.reject({code:'network',message:'fixture'})});
  for(let i=0;i<1025;i+=4) {
    const batch=Array.from({length:Math.min(4,1025-i)},(_,offset)=>acquire(image(`failure-${i+offset}`),page));
    await Promise.all(batch.map(settledImage));
    assert.ok(batch.every(lease=>lease.snapshot().reason==='network'));
    for(const lease of batch)lease.release();
  }
  assert.ok(media.imageResourceStats().failures<=1024);
  const url=image('failure-1024');media.retryImage(url,page);setPlan({fetch_image:()=>png()});
  const recovered=acquire(url,page);await flush(3);assert.equal(recovered.snapshot().status,'ready');recovered.release();
});
componentTest('封面：仅永久gone与当前owner能上报，network/direct decode/旧epoch不能清库', async()=>{
  setup();const url=image('cover');
  setPlan({fetch_image:()=>Promise.reject({code:'imageGone',message:'404 chain'}),report_broken_cover:()=>true});
  const lease=acquire(url,page);await flush(3);
  assert.equal(cover.reportCoverFailure(lease,'801',()=>false),false);
  assert.equal(cover.reportCoverFailure(lease,'801',()=>true),true);
  assert.equal(cover.reportCoverFailure(lease,'801',()=>true),false);
  await cover.awaitCoverReportFrontier();assert.equal(invokeCount('report_broken_cover'),1);
  media.resetImagesForLibraryRestore();assert.equal(cover.reportCoverFailure(lease,'802',()=>true),false);lease.release();media.resumeImages();
  const direct=acquire('https://plain.example/direct.png');direct.failDecode();
  assert.equal(cover.reportCoverFailure(direct,'803',()=>true),false);direct.release();
});
componentTest('Reader图片：原URL灯箱另持租约、HTML/video节点稳定、源码和流式零新请求', async()=>{
  setup();const url=image('reader');const html=`<p>body</p><img width="600" height="400" src="${url}"><video src="https://plain.example/video.mp4"></video>`;
  const row=mkRow({id:9801,url:page,content_html:html});seedFromRows([row]);setPlan({fetch_image:()=>png()});
  const {Reader}=await import('../../src/components/Reader.tsx');const {Lightbox}=await import('../../src/components/Overlays.tsx');
  const view=await mount(createElement('div',null,createElement(Reader),createElement(Lightbox)));
  await act(async()=>useAppStore.getState().selectArticle('9801'));await flush(5);
  const img=view.container.querySelector('.article-prose img');assert.ok(img?.src.startsWith('blob:'));
  const video=view.container.querySelector('.article-prose video');
  await act(async()=>img.dispatchEvent(new MouseEvent('click',{bubbles:true})));await flush(3);
  assert.equal(useAppStore.getState().lightboxUrl,url);assert.equal(useAppStore.getState().lightboxPageUrl,page);
  assert.equal(invokeCount('fetch_image'),1);assert.equal(view.container.querySelector('.article-prose video'),video);
  await act(async()=>useAppStore.setState({isRawRenderMode:true}));await flush(2);
  assert.ok(view.container.querySelector('.lightbox-img').src.startsWith('blob:'));
  assert.match(view.container.querySelector('.reader-source-view').textContent,/cdnfile/);
  assert.equal(invokeCount('fetch_image'),1);
  await act(async()=>useAppStore.getState().closeLightbox());
});
componentTest('播放器：常驻底栏/全屏按可见生命周期释放，空封面继承page归属', async()=>{
  setup();const url=image('player');seedFromRows([mkRow({id:9901,feed_id:13,url:page,image_url:url,enclosure_url:'https://plain.example/audio.mp3'})],{layout:'podcast'});
  setPlan({fetch_image:()=>png(),get_setting:()=>null});
  const prototype=dom.window.HTMLMediaElement.prototype;const original={play:prototype.play,pause:prototype.pause};prototype.play=()=>Promise.resolve();prototype.pause=()=>{};
  const {PlayerBar}=await import('../../src/components/PlayerBar.tsx');
  try {await mount(createElement(PlayerBar));await act(async()=>useAppStore.getState().playPodcastEpisode('A','show',url,'https://plain.example/audio.mp3','9901'));await flush(3);
    assert.equal(media.imageResourceStats().leases,1);
    await act(async()=>useAppStore.getState().togglePlayerExpanded());await flush(3);assert.equal(media.imageResourceStats().leases,2);assert.equal(invokeCount('fetch_image'),1);
    await act(async()=>useAppStore.getState().playPodcastEpisode('B','show','','https://plain.example/b.mp3','9902'));
    assert.equal(useAppStore.getState().player.coverPageUrl,page);
    await act(async()=>useAppStore.getState().closePodcastBar());await flush(3,20);assert.equal(media.imageResourceStats().leases,0);
  } finally {await unmountAll();Object.assign(prototype,original);}
});

componentTest('第257个可见封面：释放容量后自动恢复，已有租约和DOM不重建', async()=>{
  setup();setPlan({fetch_image:()=>png()});
  const existing=await mount(createElement(CoverImage,{src:image('full-shared'),pageUrl:page,className:'existing-cover',alt:'cover'}));
  const held=Array.from({length:255},()=>acquire(image('full-shared'),page));
  await flush(3);assert.equal(media.imageResourceStats().leases,256);
  const existingImg=existing.container.querySelector('img');assert.ok(existingImg?.src.startsWith('blob:'));
  const view=await mount(createElement(CoverImage,{src:image('257'),pageUrl:page,className:'test-cover',alt:'cover'}));
  await flush(2);assert.equal(invokeCount('fetch_image'),1);assert.equal(view.container.querySelector('img'),null);
  const same=held[1].snapshot();const before=useAppStore.getState().mediaCapacityRevision;
  await act(async()=>held[0].release());await flush(4);
  assert.ok(view.container.querySelector('img')?.src.startsWith('blob:'));
  assert.equal(invokeCount('fetch_image'),2);assert.equal(held[1].snapshot(),same);
  assert.equal(existing.container.querySelector('img'),existingImg);assert.equal(media.imageResourceStats().leases,256);
  assert.equal(useAppStore.getState().mediaCapacityRevision,before+1,'饱和到可用仅一个通知');
});
componentTest('直链：超过256消费者不占代理额度，满代理仍显示且本地失败可重试', async()=>{
  setup();setPlan({fetch_image:()=>png()});
  const direct=Array.from({length:300},(_,i)=>acquire(`https://plain.example/unbounded-${i}.png`));
  assert.ok(direct.every(lease=>lease.snapshot().status==='ready'));assert.equal(media.imageResourceStats().leases,0);
  assert.equal(media.imageResourceStats().ready,0);assert.equal(invokeCount('fetch_image'),0);
  const proxy=Array.from({length:256},()=>acquire(image('direct-with-full-proxy'),page));
  await settledImage(proxy[0]);
  const props={src:'https://plain.example/visible.png',className:'direct-cover',alt:'cover'};
  const view=await mount(createElement(CoverImage,props));await flush(2);
  const img=view.container.querySelector('img');assert.equal(img?.getAttribute('src'),props.src);
  await act(async()=>img.dispatchEvent(new Event('error',{bubbles:true})));await flush(2);
  assert.equal(view.container.querySelector('img'),null);assert.ok(view.container.querySelector('[data-cover-state="failed"]'));
  assert.equal(media.imageResourceStats().leases,256);assert.equal(invokeCount('report_broken_cover'),0);
  await act(async()=>view.container.querySelector('.image-retry').click());await flush(2);
  assert.equal(view.container.querySelector('img')?.getAttribute('src'),props.src);
  assert.equal(media.imageResourceStats().leases,256);assert.equal(invokeCount('fetch_image'),1);
});

componentTest('解码占位：五张25MiB退出DOM后让出预算，健康旧版与同key重试各自保留', async()=>{
  setup();const big=png(25*1024*1024);const small=png();
  setPlan({fetch_image:({url})=>url===image('decode-next')?small:big});
  const urls=Array.from({length:5},(_,i)=>image(`decode-${i}`));
  const view=await mount(createElement('div',null,...urls.map((src,i)=>createElement(CoverImage,{key:i,src,pageUrl:page,className:`decode-${i}`,alt:'cover'}))));
  await flush(6);assert.equal(view.container.querySelectorAll('img').length,5);
  assert.equal(media.imageResourceStats().blobBytes,125*1024*1024);
  const twin=acquire(urls[0],page);const old=twin.snapshot().src;
  const revoke=URL.revokeObjectURL;const revoked=[];const liveRevocations=[];
  URL.revokeObjectURL=src=>{revoked.push(src);if([...document.querySelectorAll('img')].some(img=>img.getAttribute('src')===src))liveRevocations.push(src);revoke(src);};
  try {
    await act(async()=>{for(const img of view.container.querySelectorAll('img'))img.dispatchEvent(new Event('error',{bubbles:true}));});await flush(3);
    assert.equal(view.container.querySelectorAll('img').length,0);assert.equal(view.container.querySelectorAll('[data-cover-state="failed"]').length,5);
    assert.equal(media.imageResourceStats().leases,1,'only the healthy twin still owns proxy backing');
    const next=acquire(image('decode-next'),page);await flush(4);assert.equal(next.snapshot().status,'ready');
    assert.equal(twin.snapshot().src,old);assert.ok(!revoked.includes(old));
    await act(async()=>view.container.querySelector('.decode-0 .image-retry').click());await flush(4);
    const retried=view.container.querySelector('img.decode-0');assert.ok(retried?.src.startsWith('blob:'));assert.notEqual(retried.src,old);
    assert.equal(twin.snapshot().src,old);assert.ok(!revoked.includes(old));assert.equal(invokeCount('report_broken_cover'),0);
    await act(async()=>twin.release());await flush(2);assert.ok(revoked.includes(old),'retired version releases after its last healthy owner');
    assert.ok(!revoked.includes(retried.src));assert.deepEqual(liveRevocations,[],'never revoke while any real img still references this URL');
  }finally{URL.revokeObjectURL=revoke;}
});

componentTest('Reader解码：先去src再归还lease，正文video不重建且重试不撤销健康旧版', async()=>{
  setup();setPlan({fetch_image:()=>png()});
  const {ReaderProse}=await import('../../src/components/ReaderProse.tsx');
  const url=image('reader-decode');const html=`<img src="${url}"><video src="https://plain.example/stable.mp4"></video>`;
  const props={renderHtml:html,sourceText:html,isSourceMode:false,isStreamingTranslation:false,style:{},onClick(){},imagePageUrl:page,imageOwner:'reader-decode'};
  const view=await mount(createElement(ReaderProse,props));await flush(4);
  const img=view.container.querySelector('img');const video=view.container.querySelector('video');const old=img.src;
  const twin=acquire(url,page);const revoke=URL.revokeObjectURL;const revoked=[];const liveRevocations=[];
  URL.revokeObjectURL=src=>{revoked.push(src);if(img.getAttribute('src')===src)liveRevocations.push(src);revoke(src);};
  try {
    await act(async()=>img.dispatchEvent(new Event('error',{bubbles:true})));await flush(2);
    assert.equal(img.getAttribute('src'),null);assert.equal(media.imageResourceStats().leases,1);
    assert.equal(twin.snapshot().src,old);assert.ok(!revoked.includes(old));assert.equal(view.container.querySelector('video'),video);
    const retry=view.container.querySelector('.image-retry');assert.equal(retry.hidden,false);
    await act(async()=>retry.click());await flush(4);
    assert.equal(view.container.querySelector('img'),img);assert.notEqual(img.src,old);assert.ok(img.src.startsWith('blob:'));
    assert.equal(view.container.querySelector('video'),video);assert.ok(!revoked.includes(old));
    await act(async()=>twin.release());await flush(2);assert.ok(revoked.includes(old));assert.deepEqual(liveRevocations,[]);
  }finally{URL.revokeObjectURL=revoke;}
});

componentTest('画廊解码：卡片与预览分别归还lease，重试版本不撤销健康消费者', async()=>{
  setup();const url=image('gallery-decode');const row=mkRow({id:9961,feed_id:12,title:'decode gallery',url:page,image_url:url,gallery_first_image_url:url,image_count:1,images:[url]});
  seedFromRows([row],{layout:'image'});setPlan({fetch_image:()=>png()});
  const {Timeline}=await import('../../src/components/Timeline.tsx');const {Lightbox}=await import('../../src/components/Overlays.tsx');
  const view=await mount(createElement('div',null,createElement(Timeline),createElement(Lightbox)));await flush(4);
  const trigger=view.container.querySelector('[data-id="9961"] .gallery-media-trigger');const cardImg=trigger.querySelector('img');assert.ok(cardImg?.src.startsWith('blob:'));
  await act(async()=>trigger.click());await flush(4);
  const preview=view.container.querySelector('.lightbox-img');assert.ok(preview);assert.equal(preview.src,cardImg.src);
  const twin=acquire(url,page);const old=twin.snapshot().src;const revoke=URL.revokeObjectURL;const revoked=[];const liveRevocations=[];
  URL.revokeObjectURL=src=>{revoked.push(src);if([...document.querySelectorAll('img')].some(img=>img.getAttribute('src')===src))liveRevocations.push(src);revoke(src);};
  try {
    await act(async()=>{cardImg.dispatchEvent(new Event('error',{bubbles:true}));preview.dispatchEvent(new Event('error',{bubbles:true}));});await flush(3);
    assert.equal(trigger.querySelector('img'),null);assert.equal(view.container.querySelector('.lightbox-img'),null);
    assert.equal(media.imageResourceStats().leases,1);assert.equal(twin.snapshot().src,old);assert.ok(!revoked.includes(old));
    const retry=[...view.container.querySelectorAll('button')].find(button=>button.textContent==='重试图片');assert.ok(retry);
    await act(async()=>retry.click());await flush(4);
    const retried=view.container.querySelector('.lightbox-img');assert.ok(retried?.src.startsWith('blob:'));assert.notEqual(retried.src,old);
    assert.equal(media.imageResourceStats().leases,2);assert.ok(!revoked.includes(old));
    await act(async()=>twin.release());await flush(2);assert.ok(revoked.includes(old));assert.deepEqual(liveRevocations,[]);
    await act(async()=>useAppStore.getState().closeLightbox());await flush(3);assert.equal(media.imageResourceStats().leases,0);
    assert.equal(view.container.querySelector('[data-id="9961"] .gallery-media-trigger'),trigger);
  }finally{URL.revokeObjectURL=revoke;}
});
componentTest('ReaderProse：源码/未净化流式不请求，净化译文原URL映射且无关render不重建video', async()=>{
  setup();setPlan({fetch_image:()=>png()});
  const {ReaderProse}=await import('../../src/components/ReaderProse.tsx');
  const {originalProseImage}=await import('../../src/components/useProseImages.ts');
  const url=image('translation');const html=`<p>translated</p><img src="${url}"><video src="https://plain.example/v.mp4"></video>`;
  const props={renderHtml:html,sourceText:html,isSourceMode:true,isStreamingTranslation:false,style:{fontSize:16},onClick(){},imagePageUrl:page,imageOwner:'translation'};
  const view=await mount(createElement(ReaderProse,props));await flush(2);assert.equal(invokeCount('fetch_image'),0);
  await act(async()=>view.root.render(createElement(ReaderProse,{...props,isSourceMode:false,isStreamingTranslation:true})));await flush(2);
  assert.equal(invokeCount('fetch_image'),0);assert.equal(view.container.querySelector('img'),null);
  await act(async()=>view.root.render(createElement(ReaderProse,{...props,isSourceMode:false})));await flush(4);
  const video=view.container.querySelector('video');const img=view.container.querySelector('img');
  assert.equal(originalProseImage(img).url,url);assert.ok(img.src.startsWith('blob:'));
  await act(async()=>view.root.render(createElement(ReaderProse,{...props,isSourceMode:false,style:{fontSize:18}})));
  assert.equal(view.container.querySelector('video'),video);assert.equal(invokeCount('fetch_image'),1);
});
componentTest('恢复：等待已登记坏封面写入，失败保留代际且图片服务恢复可用', async()=>{
  setup();const url=image('restore-report');const row=mkRow({id:9981,url:page,image_url:url});seedFromRows([row]);
  const report=deferred();testPending.push(report);
  setPlan({fetch_image:()=>Promise.reject({code:'imageGone',message:'404 chain'}),report_broken_cover:()=>report.promise,
    restore_portable_backup:()=>Promise.reject({code:'fixture',message:'restore failed'})});
  await mount(createElement(CoverImage,{src:url,pageUrl:page,articleId:'9981',className:'test-cover',alt:'cover'}));
  await waitFor(()=>invokeCount('report_broken_cover')===1,{label:'registered cover write'});
  await act(async()=>useAppStore.setState({portableBackup:{busy:null,preview:{path:'D:/test/backup.flxb',info:{digest:'a'.repeat(64)}},result:null,error:null,refreshFailed:false}}));
  let task;await act(async()=>{task=useAppStore.getState().restorePortableBackup();});await flush(3);
  assert.equal(invokeCount('restore_portable_backup'),0);assert.equal(media.imagesSuspended(),true);
  await act(async()=>{report.resolve(true);await task;});await flush(3);
  assert.equal(invokeCount('restore_portable_backup'),1);assert.equal(useAppStore.getState().libraryGeneration,0);
  assert.equal(media.imagesSuspended(),false);assert.equal(useAppStore.getState().entries[0].id,'9981');
  assert.equal(invokeCalls.filter(call=>call.cmd==='report_broken_cover').length,1);
});
for(const count of [24,96]) componentTest(`Reader改后对照：${count}延迟图退出仅启动4次且全部取消`, async()=>{
  setup();const pending=slowBackend();
  const html=Array.from({length:count},(_,i)=>`<p>image ${i}</p><img width="600" height="400" src="${image(`matched-${count}-${i}`)}">`).join('');
  const row=mkRow({id:9300+count,url:page,content_html:html});seedFromRows([row]);
  const {Reader}=await import('../../src/components/Reader.tsx');await mount(createElement(Reader));
  await act(async()=>useAppStore.getState().selectArticle(String(row.id)));await flush(4);
  assert.equal(invokeCount('fetch_image'),4);assert.equal(media.imageResourceStats().active,4);
  assert.ok(media.imageResourceStats().pending<=32);
  await unmountAll();await flush(4);
  assert.equal(invokeCount('cancel_image'),4);assert.equal(media.imageResourceStats().active,0);assert.equal(media.imageResourceStats().leases,0);
  assert.equal(invokeCount('fetch_image'),4,'离开后不为原96个节点继续启动请求');
  assert.equal(pending.size,4);
});


componentTest('资源React边界：SSR零acquire、StrictMode重复订阅不泄漏且render不重新取图', async()=>{
  setup();setPlan({fetch_image:()=>png()});
  const {renderToString}=await import('react-dom/server');const {StrictMode}=await import('react');
  const props={src:image('strict'),pageUrl:page,className:'test-cover',alt:'cover'};
  const ssr=renderToString(createElement(CoverImage,props));
  assert.ok(!ssr.includes('<img'));assert.equal(media.imageResourceStats().leases,0);assert.equal(invokeCount('fetch_image'),0);
  const view=await mount(createElement(StrictMode,null,createElement(CoverImage,props)));await flush(5);
  assert.equal(media.imageResourceStats().leases,1);assert.ok(view.container.querySelector('img')?.src.startsWith('blob:'));
  const count=invokeCount('fetch_image');const src=view.container.querySelector('img').src;
  await act(async()=>view.root.render(createElement(StrictMode,null,createElement(CoverImage,{...props,alt:'updated'}))));await flush(3);
  assert.equal(invokeCount('fetch_image'),count);assert.equal(view.container.querySelector('img').src,src);
  await unmountAll();await flush(3);assert.equal(media.imageResourceStats().leases,0);assert.equal(media.imageResourceStats().active,0);
});
componentTest('资源LRU：301个8KiB成功对象只保留256无人使用条目，正在显示者不撤销', async()=>{
  setup();setPlan({fetch_image:()=>png()});
  const visible=acquire(image('visible-cache'),page);await flush(2);const visibleSrc=visible.snapshot().src;
  const revoke=URL.revokeObjectURL;const revoked=[];URL.revokeObjectURL=url=>{revoked.push(url);revoke(url);};
  try {
    for(let i=0;i<301;i+=4) {
      const batch=Array.from({length:Math.min(4,301-i)},(_,offset)=>acquire(image(`cache-${i+offset}`),page));
      await Promise.all(batch.map(settledImage));
      assert.ok(batch.every(lease=>lease.snapshot().status==='ready'));
      for(const lease of batch)lease.release();
    }
    const stats=media.imageResourceStats();assert.equal(stats.ready,257);assert.equal(stats.idleBytes,256*8192);
    assert.equal(visible.snapshot().src,visibleSrc);assert.ok(!revoked.includes(visibleSrc));assert.equal(revoked.length,45);
  }finally{URL.revokeObjectURL=revoke;}
});


