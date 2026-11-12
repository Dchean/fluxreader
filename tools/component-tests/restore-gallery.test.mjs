// 恢复与基础批画廊的共同边界；真实 store/Lightbox，只控制外部 IPC 数据与时序。
// jsdom 的焦点与组件状态不替代 Windows 原生选择器或真实图片布局验收。
import assert from 'node:assert/strict';
import { componentTest, mount, unmountAll, flush, resetStore, useAppStore } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, setPlan, resetPlan, invokeCount, invokeCalls, deferred, setCurrentRows } from './fixtures.mjs';

const { act, createElement } = await import('react');
const { Lightbox } = await import('../../src/components/Overlays.tsx');
const { api, awaitLocalFlagWriteFrontier } = await import('../../dist-test/lib/api.js');
const anchors = await import('../../dist-test/lib/readingPosition.js');
const state = () => useAppStore.getState();
const info = { path: 'D:/tests/gallery-restore.flxb', size_bytes: 100, digest: 'c'.repeat(64), sources: 1, articles: 1, starred: 0, missing_bodies: 0 };
const localStatus = { connected: false, endpoint: null, username: null, mode: 'scheduled', interval_minutes: 15, generation: 1, library_id: 'restored', pending_operations: 0, pending_bodies: 0, pending_publications: 0, last_state_sync: 0, last_body_sync: 0, next_retry_at: 0, last_error: null };
const imageRow = (id, images, extra = {}) => mkRow({
  id, feed_id: 12, title: `画廊条目 ${id}`, url: `https://gallery.example/${id}`,
  image_url: images[0] ?? null, gallery_first_image_url: images[0] ?? null,
  image_count: images.length, images, ...extra,
});
const restoredRow = () => imageRow(99001, ['https://img.example/new-1.png', 'https://img.example/new-2.png']);

// 先结清本例实际挂起的任务，再卸载并还原设置；不能在harness等待前沿之后才释放gate。
const galleryRestoreTest = (name, body) => componentTest(name, { timeout: 20000 }, async () => {
  const before = { settings: state().settings, settingsTab: state().settingsTab };
  const gates = [], tasks = [], errors = [];
  const resource = {
    defer(fallback = null) { const gate = deferred(); gates.push({ gate, fallback }); return gate; },
    track(task) { tasks.push(task); return task; },
  };
  try { await body(resource); } catch (error) { errors.push(error); }
  finally {
    try {
      await act(async () => {
        for (const { gate, fallback } of gates) gate.resolve(fallback);
        const results = await Promise.allSettled(tasks);
        for (const result of results) if (result.status === 'rejected') errors.push(result.reason);
        await awaitLocalFlagWriteFrontier();
      });
    } catch (error) { errors.push(error); }
    try { await unmountAll(); } catch (error) { errors.push(error); }
    try { await act(async () => { state().closeLightbox(); useAppStore.setState(before); }); } catch (error) { errors.push(error); }
    resetPlan();
  }
  if (errors.length === 1) throw errors[0];
  if (errors.length > 1) throw new AggregateError(errors, '恢复画廊用例及局部清理失败');
});

async function setup(rows, overrides = {}, { timeline = false, markReadOnOpen = true } = {}) {
  installBackend(); resetPlan(); resetStore(); seedFromRows(rows, { layout: 'image' });
  useAppStore.setState((s) => ({ timelineFilter: 'all', settings: { ...s.settings, markReadOnOpen },
    portableBackup: { busy: null, preview: { path: info.path, info: { digest: info.digest } }, result: null, error: null, refreshFailed: false } }));
  setPlan({
    local_sync_status: () => Promise.resolve(localStatus),
    restore_portable_backup: () => { setCurrentRows([restoredRow()]); return Promise.resolve(info); },
    ...overrides,
  });
  const { Timeline } = await import('../../src/components/Timeline.tsx');
  const view = await mount(createElement('div', null,
    createElement('button', { 'data-open-focus': true }, '打开图片'),
    timeline && createElement(Timeline), createElement(Lightbox)));
  await flush(2);
  return {
    ...view,
    async open(id) {
      await act(async () => { state().selectArticle(String(id)); state().openGalleryLightbox(String(id)); });
      await flush(2);
    },
  };
}

function assertClosed(view) {
  assert.equal(state().lightboxGallery, null);
  assert.equal(state().lightboxUrl, null);
  assert.equal(state().lightboxEntryId, null);
  assert.equal(view.container.querySelector('.lightbox-overlay').classList.contains('open'), false);
}

async function assertCommitted(task) {
  await act(async () => { await task; }); await flush(3);
  assert.equal(state().libraryRestoreBusy, false);
  assert.equal(state().libraryGeneration, 1);
  assert.equal(state().portableBackup.result?.kind, 'restore');
  assert.equal(state().portableBackup.refreshFailed, false, state().portableBackup.error ?? '');
  assert.deepEqual(state().entries.map((entry) => entry.id), ['99001']);
}

galleryRestoreTest('恢复画廊：无首图请求在busy开始失效，等待既有收藏前沿期间不得补发标读', async ({ defer, track }) => {
  const old = imageRow(8101, []), detail = defer(), flag = defer();
  const view = await setup([old], { get_article: () => detail.promise, set_starred: () => flag.promise });
  await view.open(old.id);
  assert.equal(state().lightboxGallery.status, 'loading');
  assert.equal(state().lightboxGallery.opened, false);
  assert.equal(invokeCount('set_read'), 0);
  const flagWrite = track(api.setStarred(old.id, true));
  await flush(2);
  let task;
  await act(async () => {
    task = track(state().restorePortableBackup());
    assert.equal(state().libraryRestoreBusy, true);
    assert.equal(state().lightboxGallery, null, '不等任何Promise才关闭');
    state().openGalleryLightbox(String(old.id));
    state().openLightbox('https://img.example/forbidden.png');
    state().markEntryOpened(String(old.id));
  });
  await flush(3);
  assert.equal(invokeCount('restore_portable_backup'), 0, '已有收藏写仍未结清');
  assert.equal(invokeCount('get_article'), 1, 'busy期间不开新图片详情');
  await act(async () => { detail.resolve({ ...old, images: ['https://img.example/late.png'] }); });
  await flush(3);
  assertClosed(view);
  assert.equal(invokeCount('set_read'), 0, '迟到详情不能越过已捕获读藏前沿补发新标读');
  assert.equal(state().entries[0].isRead, false, '也不能只改乐观读态');
  assert.equal(invokeCount('restore_portable_backup'), 0);
  await act(async () => { flag.resolve(null); await flagWrite; });
  await assertCommitted(task);
  assertClosed(view);
  assert.equal(invokeCount('restore_portable_backup'), 1);
});

galleryRestoreTest('恢复画廊：已打开图片立即关闭，既有标读完整结清，成功后新库图片可正常标读', async ({ defer, track }) => {
  const old = imageRow(8102, ['https://img.example/old.png']), detail = defer(), read = defer();
  const view = await setup([old], { get_article: () => detail.promise, set_read: () => read.promise });
  await view.open(old.id);
  const requestId = state().lightboxGallery.requestId;
  assert.equal(state().lightboxUrl, old.images[0]);
  assert.equal(invokeCount('set_read'), 1);
  let task;
  await act(async () => { task = track(state().restorePortableBackup()); }); await flush(3);
  assertClosed(view);
  assert.equal(invokeCount('restore_portable_backup'), 0, '不能清空旧标读前沿来恢复');
  await act(async () => { detail.resolve(old); }); await flush(2);
  assertClosed(view);
  await act(async () => { read.resolve([{ id: old.id, feed_id: old.feed_id }]); });
  await assertCommitted(task);
  setPlan({ get_article: () => Promise.resolve(restoredRow()), set_read: () => Promise.resolve(null) });
  await view.open(99001);
  assert.ok(state().lightboxGallery.requestId > requestId, '成功重建也不重用请求序号');
  assert.deepEqual(state().lightboxGallery.urls, restoredRow().images);
  assert.deepEqual(invokeCalls.filter((call) => call.cmd === 'set_read').map((call) => call.args.id), [old.id, 99001]);
});

for (const outcome of ['success', 'empty', 'reject']) galleryRestoreTest(`恢复画廊：提交后旧详情${outcome}不能抢新集合、选择或焦点`, async ({ defer, track }) => {
  const old = imageRow(8201, []), detail = defer();
  const view = await setup([old], { get_article: () => detail.promise });
  await view.open(old.id);
  const previousRequest = state().lightboxGallery.requestId;
  let task;
  await act(async () => { task = track(state().restorePortableBackup()); });
  await assertCommitted(task); assertClosed(view);
  setPlan({ get_article: () => Promise.resolve(restoredRow()) });
  await view.open(99001);
  const gallery = state().lightboxGallery;
  const next = view.container.querySelector('.lightbox-next');
  assert.ok(gallery.requestId > previousRequest);
  next.focus();
  const reads = invokeCount('set_read');
  await act(async () => {
    if (outcome === 'reject') detail.reject(new Error('旧库图片详情失败'));
    else detail.resolve({ ...old, images: outcome === 'empty' ? [] : ['https://img.example/stale.png'] });
  });
  await flush(3);
  assert.equal(state().lightboxGallery, gallery);
  assert.equal(state().lightboxUrl, restoredRow().images[0]);
  assert.equal(state().activeArticleId, '99001');
  assert.equal(document.activeElement, next);
  assert.equal(invokeCount('set_read'), reads);
  await act(async () => { next.click(); });
  assert.equal(state().lightboxUrl, restoredRow().images[1], '旧回包后当前画廊仍可操作');
});

for (const initiallyOpened of [false, true]) galleryRestoreTest(`恢复画廊：失败保旧库且不自动复活${initiallyOpened ? '已打开' : '无首图'}请求，明确重开才显示`, async ({ defer, track }) => {
  const images = ['https://img.example/old-ready.png'];
  const old = imageRow(8301, initiallyOpened ? images : []), detail = defer(), restore = defer();
  const view = await setup([old], { get_article: () => detail.promise, restore_portable_backup: () => restore.promise });
  await view.open(old.id);
  const requestId = state().lightboxGallery.requestId;
  const memberIds = state().entries.map((entry) => entry.id);
  let task;
  await act(async () => { task = track(state().restorePortableBackup()); }); await flush(3);
  assertClosed(view);
  await act(async () => { restore.reject({ code: 'staleBackupPreview', message: '未提交恢复' }); await task; });
  assert.equal(state().libraryRestoreBusy, false);
  assert.equal(state().libraryGeneration, 0);
  assert.deepEqual(state().entries.map((entry) => entry.id), memberIds);
  assert.equal(state().activeArticleId, String(old.id));
  assert.equal(state().portableBackup.result, null);
  assert.match(state().portableBackup.error, /恢复失败/);
  const focus = view.container.querySelector('[data-open-focus]'); focus.focus();
  const reads = invokeCount('set_read');
  await act(async () => { detail.resolve({ ...old, images }); }); await flush(3);
  assertClosed(view);
  assert.equal(document.activeElement, focus);
  assert.equal(invokeCount('set_read'), reads);
  setPlan({ get_article: () => Promise.resolve({ ...old, images }) });
  await view.open(old.id);
  assert.ok(state().lightboxGallery.requestId > requestId);
  assert.equal(state().lightboxUrl, images[0]);
  assert.equal(state().lightboxGallery.status, 'ready');
  assert.equal(invokeCount('set_read'), 1, '原库仍可读；已标读不重复，未标读明确重开后才写');
});

galleryRestoreTest('恢复画廊：成功reset本身调用真实关闭动作，再次同ID打开不复用旧请求', async ({ defer }) => {
  const old = imageRow(8401, []), detail = defer();
  const view = await setup([old], { get_article: () => detail.promise });
  await view.open(old.id);
  const requestId = state().lightboxGallery.requestId;
  const { resetLibraryViewAfterRestore } = await import('../../dist-test/store/slices/bootstrap.js');
  await act(async () => { resetLibraryViewAfterRestore(); });
  assertClosed(view);
  assert.equal(state().libraryGeneration, 1);
  await act(async () => { seedFromRows([old], { layout: 'image' }); });
  setPlan({ get_article: () => Promise.resolve({ ...old, images: ['https://img.example/reopened.png'] }) });
  await view.open(old.id);
  assert.ok(state().lightboxGallery.requestId > requestId);
  const current = state().lightboxGallery;
  await act(async () => { detail.resolve({ ...old, images: ['https://img.example/stale.png'] }); });
  await flush(2);
  assert.equal(state().lightboxGallery, current);
  assert.equal(state().lightboxUrl, 'https://img.example/reopened.png');
});

galleryRestoreTest('恢复画廊：提交后刷新重试先失效新请求，只刷新不重复恢复或推进库代际', async ({ defer, track }) => {
  const view = await setup([imageRow(8451, [])], {
    get_setting: ({ key }) => key === 'app_settings' ? Promise.reject(new Error('恢复后设置回读失败')) : Promise.resolve(null),
  });
  await act(async () => { await state().restorePortableBackup(); }); await flush(3);
  assert.equal(state().portableBackup.result?.kind, 'restore');
  assert.equal(state().portableBackup.refreshFailed, true);
  assert.equal(state().libraryGeneration, 1);
  const detail = defer(), settings = defer('{}');
  setPlan({ get_article: () => detail.promise,
    get_setting: ({ key }) => key === 'app_settings' ? settings.promise : Promise.resolve(null) });
  await view.open(99001);
  assert.ok(state().lightboxGallery);
  let retry;
  await act(async () => {
    retry = track(state().retryRestoredLibraryRefresh());
    assert.equal(state().libraryRestoreBusy, true);
    assert.equal(state().lightboxGallery, null);
  });
  await act(async () => { detail.resolve(restoredRow()); }); await flush(3);
  assertClosed(view);
  await act(async () => { settings.resolve('{}'); await retry; }); await flush(3);
  assert.equal(state().portableBackup.refreshFailed, false, state().portableBackup.error ?? '');
  assert.equal(state().libraryRestoreBusy, false);
  assert.equal(state().libraryGeneration, 1);
  assert.equal(invokeCount('restore_portable_backup'), 1);
  assertClosed(view);
});

galleryRestoreTest('恢复画廊：普通WebDAV状态/属性回读保固定成员、滚动位置与当前切图', async () => {
  const old = imageRow(8501, ['https://img.example/sync-1.png', 'https://img.example/sync-2.png']);
  const view = await setup([old], {}, { timeline: true, markReadOnOpen: false });
  await act(async () => { await state().reloadFromBackend(); }); await flush(3);
  await view.open(old.id);
  const session = state().readingSession;
  const requestId = state().lightboxGallery.requestId;
  const scroller = view.container.querySelector('#timelineContentScroll');
  scroller.scrollTop = 173;
  const filterKey = 'image|all|all|all|newest';
  anchors.rearmTopAnchor(String(old.id), filterKey, performance.now(), 13);
  anchors.stashTopAnchorForReturn();
  const anchor = anchors.peekReturnAnchor(filterKey);
  const membersBefore = invokeCount('list_article_member_ids');
  const connected = { ...localStatus, connected: true, last_state_sync: 1234 };
  setPlan({ local_sync_run: () => Promise.resolve(connected), local_sync_status: () => Promise.resolve(connected) });
  setCurrentRows([{ ...old, is_starred: true }, imageRow(8502, ['https://img.example/new-member.png'])]);
  await act(async () => { await state().runLocalSync(); await state().reloadFromBackend({ keepReadingPosition: true }); });
  await flush(3);
  assert.equal(invokeCount('local_sync_run'), 1);
  assert.equal(state().libraryGeneration, 0);
  assert.equal(state().readingSession.generation, session.generation);
  assert.deepEqual(state().readingSession.memberIds, session.memberIds);
  assert.equal(invokeCount('list_article_member_ids'), membersBefore);
  assert.equal(state().entries[0].isStarred, true);
  assert.equal(state().activeArticleId, String(old.id));
  assert.equal(state().lightboxGallery.requestId, requestId);
  assert.equal(scroller.scrollTop, 173);
  assert.deepEqual(anchors.peekReturnAnchor(filterKey), anchor);
  await act(async () => { view.container.querySelector('.lightbox-next').click(); });
  assert.equal(state().lightboxUrl, old.images[1]);
});
