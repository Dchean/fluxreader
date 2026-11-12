// 数据安全 UI：手动数据库备份、正文保留设置与缓存清理语义。
//
// 真实挂载生产组件，假后端只替换 IPC 返回/拒绝：
// - 手动备份成功显示路径、版本与大小，并可从真实按钮在文件夹中显示；
// - 备份失败显示错误且不伪装成功；
// - 自动保留只提交有效天数，草稿不被回读覆盖，失败回滚且不跨恢复代次写入；
// - 清理确认文案明确「只删已读/未收藏/未待同步」「未读不清理」「远端可能回灌」。

import assert from 'node:assert/strict';
import {
  componentTest, mount, unmountAll, flush, waitFor, resetStore, useAppStore,
} from './harness.mjs';
import {
  installBackend, clearInvokeCalls, setPlan, resetPlan, invokeCount, invokeCalls, deferred,
} from './fixtures.mjs';

const { act, createElement } = await import('react');
const { CacheCleanupSection } = await import('../../src/components/settings/CacheCleanupSection.tsx');
const {
  awaitAppSettingsWriteFrontier, resetAppSettingsWriteStateForTests, resetAppSettingsForLibraryRestore,
} = await import('../../dist-test/store/slices/settings.js');

const BACKUP = {
  path: 'C:\\Users\\tester\\AppData\\Roaming\\com.fluxreader.app\\backups\\manual-v34-to-v34-test.db',
  fileName: 'manual-v34-to-v34-test.db',
  sizeBytes: 4096,
  schemaVersion: 34,
  fromVersion: 34,
  toVersion: 34,
  createdAt: '2026-11-04T10:00:00.000Z',
  kind: 'manual',
};

function findButton(root, text) {
  return [...root.querySelectorAll('button')].find((b) => b.textContent.trim() === text);
}

componentTest('手动数据库备份：成功显示路径/版本/大小并可 reveal，失败可见', { timeout: 40000 }, async (t) => {
  installBackend();
  clearInvokeCalls();
  resetPlan();
  resetStore();
  useAppStore.setState({ dataMode: 'tauri' });
  t.after(() => {
    resetPlan();
    globalThis.__REVEALED_ITEMS__ = undefined;
  });

  setPlan({ create_database_backup: () => Promise.resolve(BACKUP) });
  const { createElement, act } = await import('react');
  const { DatabaseBackupSection } = await import('../../src/components/settings/DatabaseBackupSection.tsx');
  const entry = await mount(createElement(DatabaseBackupSection));
  await flush(2);

  const create = findButton(entry.container, '创建数据库备份');
  assert.ok(create, '真实「创建数据库备份」按钮存在');
  await act(async () => { create.click(); });
  await waitFor(
    () => entry.container.querySelector('.database-backup-path')?.textContent === BACKUP.path,
    { label: '成功路径进入真实 DOM', timeout: 6000 },
  );
  assert.equal(invokeCount('create_database_backup'), 1, '点击真实调用备份命令');
  const result = entry.container.querySelector('.database-backup-result');
  assert.ok(result?.textContent.includes('v34 → v34'), '显示备份 schema 版本');
  assert.ok(result?.textContent.includes('4.0 KB'), '显示备份实际大小');
  assert.equal(
    result?.querySelector('.database-backup-path')?.textContent,
    BACKUP.path,
    '路径逐字显示',
  );

  const reveal = findButton(entry.container, '在文件夹中显示');
  assert.ok(reveal, '成功卡提供文件夹入口');
  await act(async () => { reveal.click(); });
  await flush(2);
  assert.deepEqual(globalThis.__REVEALED_ITEMS__, [BACKUP.path], 'reveal 真实收到备份路径');

  setPlan({
    create_database_backup: () => Promise.reject({ code: 'backup', message: '磁盘空间不足' }),
  });
  const createAgain = findButton(entry.container, '创建数据库备份');
  await act(async () => { createAgain.click(); });
  await waitFor(
    () => entry.container.querySelector('.database-backup-error')?.textContent?.includes('磁盘空间不足'),
    { label: '备份失败错误可见', timeout: 6000 },
  );
  assert.ok(
    useAppStore.getState().toasts.some((toast) => toast.text.includes('备份失败：磁盘空间不足')),
    '失败同时进入 toast，不伪装成功',
  );
});

componentTest('缓存清理确认：已读/未收藏/未待同步边界、未读保留与远端回灌说明', { timeout: 40000 }, async (t) => {
  installBackend();
  clearInvokeCalls();
  resetPlan();
  resetStore();
  const beforeSettings = useAppStore.getState().settings;
  useAppStore.setState({ dataMode: 'tauri', settings: { ...beforeSettings, readBodyRetentionDays: 0 } });
  t.after(() => { resetPlan(); useAppStore.setState({ settings: beforeSettings }); });
  setPlan({ cache_cleanup: () => Promise.resolve('已清理 30 天前的已读文章 1 篇（未读/收藏/待同步保留）') });

  const { createElement, act } = await import('react');
  const { CacheCleanupSection } = await import('../../src/components/settings/CacheCleanupSection.tsx');
  const entry = await mount(createElement(CacheCleanupSection));
  await flush(2);

  assert.ok(entry.container.querySelector('[role="combobox"][aria-label="已读正文保留"]'), '自动保留模式入口可用');
  assert.ok(entry.container.textContent.includes('永久保留'), '默认永久保留与手动清理分别表达');

  const cleanup = findButton(entry.container, '释放旧正文');
  assert.ok(cleanup, '真实「释放旧正文」按钮存在');
  await act(async () => { cleanup.click(); });
  const dialog = document.body.querySelector('.confirm-overlay .confirm-dialog');
  assert.ok(dialog, '确认框真实挂载');
  const message = dialog.querySelector('.confirm-message')?.textContent ?? '';
  assert.ok(message.includes('发表于 30 天前'), `手动清理按发表时间，不冒充自动保留期: ${message}`);
  assert.ok(message.includes('已读、未收藏且无待同步状态'), `确认框说明清理条件: ${message}`);
  assert.ok(message.includes('未读与收藏正文保留'), `确认框说明未读保留: ${message}`);
  assert.ok(message.includes('远端仍有的正文可能重新获取'), `确认框说明远端回灌: ${message}`);
  assert.ok(message.includes('此操作不可撤销'), `确认框保留不可撤销语义: ${message}`);

  const confirm = findButton(dialog, '确认清理');
  assert.ok(confirm, '确认按钮存在');
  await act(async () => { confirm.click(); });
  await waitFor(
    () => useAppStore.getState().toasts.some((toast) => toast.text.includes('已读文章 1 篇')),
    { label: '清理回执显示真实保留条件', timeout: 6000 },
  );
  assert.deepEqual(
    invokeCalls.filter((call) => call.cmd === 'cache_cleanup').map((call) => call.args),
    [{ days: 30, scope: 'articles' }],
    '真实调用缓存清理命令（默认 30 天、文章范围）',
  );
});

// 只替换 IPC 的返回/拒绝时机，保存仍走生产 updateSettings 与原子 patch 链。
// 每例先排空设置前沿、复位确认基线，结束时释放自有延迟并恢复进入前设置。
function retentionTest(name, body, initialDays = 0) {
  componentTest(name, { timeout: 40000 }, async () => {
    const before = {
      settings: { ...useAppStore.getState().settings },
      activeViewFilter: useAppStore.getState().activeViewFilter,
      timelineFilter: useAppStore.getState().timelineFilter,
      libraryGeneration: useAppStore.getState().libraryGeneration,
      libraryRestoreBusy: useAppStore.getState().libraryRestoreBusy,
    };
    const pending = [];
    let db = {};
    try {
      await act(async () => { await awaitAppSettingsWriteFrontier(); });
      resetAppSettingsWriteStateForTests();
      installBackend(); resetPlan(); resetStore();
      await act(async () => {
        useAppStore.setState({ dataMode: 'tauri' });
        resetAppSettingsForLibraryRestore();
      });
      assert.equal(useAppStore.getState().settings.readBodyRetentionDays, 0, '生产默认永久保留');
      db = { ...useAppStore.getState().settings, readBodyRetentionDays: initialDays };
      const readSettings = ({ key }) => Promise.resolve(key === 'app_settings' ? JSON.stringify(db) : null);
      const persistPatch = ({ patch }) => {
        db = { ...db, ...patch };
        return Promise.resolve(JSON.stringify(db));
      };
      setPlan({ get_setting: readSettings, patch_app_settings: persistPatch });
      await act(async () => { await useAppStore.getState().bootstrapSettings(); });
      clearInvokeCalls();
      const entry = await mount(createElement(CacheCleanupSection));
      await flush(2);
      await body({
        entry, readSettings, persistPatch,
        readDb: () => db,
        changeDb: (patch) => { db = { ...db, ...patch }; },
        delay: () => { const gate = deferred(); pending.push(gate); return gate; },
      });
    } finally {
      await act(async () => {
        for (const gate of pending) gate.resolve(JSON.stringify(db));
        await awaitAppSettingsWriteFrontier();
      });
      await flush(2);
      await unmountAll();
      resetAppSettingsWriteStateForTests(); resetPlan();
      await act(async () => { useAppStore.setState(before); });
    }
  });
}

const retentionInput = (root) => root.querySelector('[aria-label="已读正文保留天数"]');
const retentionMode = (root) => root.querySelector('[role="combobox"][aria-label="已读正文保留"]');
const retentionPatches = () => invokeCalls.filter((call) => call.cmd === 'patch_app_settings').map((call) => call.args.patch);

async function editRetention(input, value) {
  assert.ok(input, '真实天数输入存在');
  await act(async () => {
    input.focus();
    Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set.call(input, value);
    input.dispatchEvent(new window.Event('input', { bubbles: true }));
  });
}

async function enterRetention(input, isComposing = false) {
  await act(async () => {
    input.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Enter', bubbles: true, isComposing }));
  });
}

async function chooseRetention(root, label) {
  const trigger = retentionMode(root);
  assert.ok(trigger, '真实模式控件存在');
  await act(async () => { trigger.focus(); trigger.click(); });
  const menu = document.getElementById(trigger.getAttribute('aria-controls'));
  const option = [...menu.querySelectorAll('[role="option"]')].find((item) => item.textContent.trim() === label);
  assert.ok(option, `真实选项存在：${label}`);
  await act(async () => { option.click(); });
  await flush(2);
}

retentionTest('正文保留：默认永久，模式立即原子保存且保留远端别键，不触发手动清理', async ({ entry, changeDb, readDb }) => {
  assert.match(retentionMode(entry.container).textContent, /永久保留/);
  assert.equal(retentionInput(entry.container), null);
  assert.match(entry.container.textContent, /未读与收藏的已有正文长期保留/);
  assert.equal(invokeCount('patch_app_settings'), 0, '仅显示设置不写库');
  changeDb({ fontSize: 19 });

  await chooseRetention(entry.container, '限定天数');
  assert.equal(retentionInput(entry.container).value, '30');
  assert.doesNotMatch(
    entry.container.textContent,
    /已读且未收藏时开始计时；重新取回正文后重新计时/,
    '用途说明小字必须从界面移除',
  );
  assert.deepEqual(retentionPatches(), [{ readBodyRetentionDays: 30 }]);
  assert.equal(readDb().fontSize, 19, '别设备已保存的无关设置未被旧快照覆盖');
  assert.equal(useAppStore.getState().settings.fontSize, 19, '原子写回完整真值进入界面投影');

  await chooseRetention(entry.container, '永久保留');
  assert.equal(retentionInput(entry.container), null);
  assert.deepEqual(retentionPatches(), [{ readBodyRetentionDays: 30 }, { readBodyRetentionDays: 0 }]);
  assert.equal(invokeCount('set_setting'), 0, '控件不绕开原子 patch 写全量');
  assert.equal(invokeCount('cache_cleanup'), 0, '改变自动期限不执行手动清理命令');
});

retentionTest('正文保留：可删除重输，Enter/失焦各保存一次，无效草稿可见纠正且边界有效', async ({ entry }) => {
  const input = retentionInput(entry.container);
  await editRetention(input, '');
  assert.equal(input.value, '', '删除可保持空草稿');
  await editRetention(input, '7');
  assert.deepEqual(retentionPatches(), [], '输入中不提交');
  await enterRetention(input);
  await act(async () => { input.blur(); });
  assert.deepEqual(retentionPatches(), [{ readBodyRetentionDays: 7 }], 'Enter 后失焦不重复保存');

  for (const text of ['', '0', '-1', '3651', '1.5', 'abc']) {
    await editRetention(input, text);
    if (text === '') await act(async () => { input.blur(); });
    else await enterRetention(input);
    assert.equal(input.value, text, '无效输入保留供纠正');
    assert.equal(input.getAttribute('aria-invalid'), 'true');
    const error = document.getElementById(input.getAttribute('aria-describedby'));
    assert.equal(error?.textContent, '请输入 1–3650 的整数天数');
    assert.equal(error?.getAttribute('role'), 'alert', '校验信息与输入关联且可读屏');
    assert.equal(retentionPatches().length, 1, '无效中间值不落库');
    assert.equal(useAppStore.getState().settings.readBodyRetentionDays, 7);
  }

  await editRetention(input, '1');
  assert.equal(input.hasAttribute('aria-invalid'), false, '修改即允许纠正旧错误');
  await enterRetention(input);
  await editRetention(input, '3650');
  await act(async () => { input.blur(); });
  await editRetention(input, '42');
  await enterRetention(input, true);
  assert.equal(retentionPatches().length, 3, '输入法确认未被抢作设置提交');
  await enterRetention(input);
  assert.deepEqual(retentionPatches(), [
    { readBodyRetentionDays: 7 }, { readBodyRetentionDays: 1 },
    { readBodyRetentionDays: 3650 }, { readBodyRetentionDays: 42 },
  ]);
}, 30);

retentionTest('正文保留：模式与天数保存失败均回滚到已保存值并提示，不遗留成功假象', async ({ entry, persistPatch, delay }) => {
  setPlan({ patch_app_settings: () => Promise.reject({ message: '设置磁盘只读' }) });
  await chooseRetention(entry.container, '限定天数');
  assert.match(retentionMode(entry.container).textContent, /永久保留/);
  assert.equal(retentionInput(entry.container), null, '失败模式恢复永久');
  assert.ok(useAppStore.getState().toasts.some((toast) => toast.text.includes('设置保存失败：设置磁盘只读')));

  setPlan({ patch_app_settings: persistPatch });
  await chooseRetention(entry.container, '限定天数');
  const save = delay();
  setPlan({ patch_app_settings: () => save.promise });
  const input = retentionInput(entry.container);
  await editRetention(input, '60');
  await enterRetention(input);
  assert.equal(input.value, '60', '提交后沿用既有乐观投影');
  await act(async () => { save.reject({ message: '天数写入失败' }); await awaitAppSettingsWriteFrontier(); });
  assert.equal(input.value, '30', '失败回读投影覆盖已提交草稿');
  assert.equal(useAppStore.getState().settings.readBodyRetentionDays, 30);
  assert.ok(useAppStore.getState().toasts.some((toast) => toast.text.includes('设置保存失败：天数写入失败')));
  assert.equal(invokeCount('cache_cleanup'), 0);
});

retentionTest('正文保留：真实设置回读保住活动草稿，单纯聚焦跟随新真值且不反写旧值', async ({ entry, changeDb, readDb, readSettings, delay }) => {
  const input = retentionInput(entry.container);
  await act(async () => { input.focus(); });
  changeDb({ readBodyRetentionDays: 60 });
  await act(async () => { await useAppStore.getState().bootstrapSettings(); });
  assert.equal(input.value, '60', '只聚焦而未编辑时仍跟随真实回读');
  await act(async () => { input.blur(); });
  assert.deepEqual(retentionPatches(), [], '没有草稿不把聚焦前的旧值写回');

  await editRetention(input, '');
  await editRetention(input, '45');
  const read = delay();
  setPlan({ get_setting: () => read.promise });
  let readback;
  await act(async () => { readback = useAppStore.getState().bootstrapSettings(); });
  changeDb({ readBodyRetentionDays: 0, fontSize: 21 });
  await act(async () => { read.resolve(JSON.stringify(readDb())); await readback; });
  assert.equal(useAppStore.getState().settings.readBodyRetentionDays, 0, '真实回读已更新持久真值');
  assert.equal(input.value, '45', '回读不吞正在编辑的草稿');
  assert.match(retentionMode(entry.container).textContent, /限定天数/, '草稿仍有可纠正入口');
  setPlan({ get_setting: readSettings });
  await enterRetention(input);
  await act(async () => { input.blur(); });
  assert.deepEqual(retentionPatches(), [{ readBodyRetentionDays: 45 }]);
  assert.equal(readDb().fontSize, 21);
  assert.equal(readDb().readBodyRetentionDays, 45);
}, 30);

retentionTest('正文保留：编辑天数后选择永久，只保存永久，不先落短期草稿', async ({ entry }) => {
  const input = retentionInput(entry.container);
  await editRetention(input, '1');
  await chooseRetention(entry.container, '永久保留');
  assert.deepEqual(retentionPatches(), [{ readBodyRetentionDays: 0 }], '转到模式按钮的失焦不先提交短期天数');
  assert.equal(retentionInput(entry.container), null);
  assert.match(retentionMode(entry.container).textContent, /永久保留/);
}, 30);

retentionTest('正文保留：失焦前恢复忙态或换库代次推进，旧草稿不能写进恢复中的资料库', async ({ entry }) => {
  let input = retentionInput(entry.container);
  await editRetention(input, '2');
  await act(async () => {
    useAppStore.setState({ libraryRestoreBusy: true });
    input.blur(); // 同一事件窗口：React 重绘前的旧闭包也须检查当前 busy。
  });
  assert.deepEqual(retentionPatches(), []);
  assert.equal(input.disabled, true);
  await enterRetention(input);
  await chooseRetention(entry.container, '永久保留');
  assert.deepEqual(retentionPatches(), [], '已打开菜单/程序分发事件也不能绕过恢复忙态');

  await act(async () => { useAppStore.setState({ libraryRestoreBusy: false }); });
  await editRetention(input, '3');
  const oldInput = input;
  await act(async () => {
    useAppStore.setState((state) => ({
      libraryGeneration: state.libraryGeneration + 1,
      settings: { ...state.settings, readBodyRetentionDays: 90 },
    }));
    oldInput.blur(); // 仍是旧 DOM 和旧草稿，但全局代次已经变化。
  });
  input = retentionInput(entry.container);
  assert.deepEqual(retentionPatches(), [], '代次变化后的旧失焦事件不写旧天数');
  assert.notEqual(input, oldInput, '新库使用新的输入元素');
  assert.equal(input.value, '90', '换库后显示新设置，不沿用旧草稿');
  assert.equal(input.disabled, false);
  assert.equal(useAppStore.getState().settings.readBodyRetentionDays, 90);
}, 30);

retentionTest('缓存清理：事务成功后回读失败，真实重试按钮只刷新，不重复释放正文', async ({ entry }) => {
  const { ErrorCenterDialog } = await import('../../src/components/ErrorCenter.tsx');
  const errorEntry = await mount(createElement(ErrorCenterDialog));
  let failed = true;
  setPlan({
    cache_cleanup: () => Promise.resolve('旧正文已释放'),
    list_folders: () => failed ? Promise.reject({ message: '目录回读失败' }) : Promise.resolve([]),
  });
  await act(async () => { findButton(entry.container, '释放旧正文').click(); });
  const dialog = document.body.querySelector('.confirm-overlay .confirm-dialog');
  assert.match(dialog.querySelector('.confirm-message').textContent, /发表于 30 天前/);
  await act(async () => { findButton(dialog, '确认清理').click(); });
  await waitFor(() => useAppStore.getState().errors.some((error) => error.action.label === '重试界面刷新'), { label: '清理成功/回读失败重试入口' });
  await act(async () => { useAppStore.getState().openErrorCenter(); });
  assert.ok(findButton(errorEntry.container, '重试界面刷新'), '真实错误中心保留刷新重试按钮');
  assert.ok(useAppStore.getState().toasts.some((toast) => toast.text.includes('清理已完成，但界面刷新失败：目录回读失败')));
  const readsBeforeRetry = invokeCount('list_folders');
  failed = false;
  await act(async () => { findButton(errorEntry.container, '重试界面刷新').click(); });
  await flush(4);
  assert.equal(invokeCount('list_folders'), readsBeforeRetry + 1, '点击真实按钮重新回读');
  assert.deepEqual(
    invokeCalls.filter((call) => call.cmd === 'cache_cleanup').map((call) => call.args),
    [{ days: 30, scope: 'articles' }],
    '刷新重试不重复已完成的数据操作',
  );
  assert.equal(invokeCount('patch_app_settings'), 0, '手动清理不改变自动保留设置');
});
