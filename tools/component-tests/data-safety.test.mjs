// 数据安全 UI：手动数据库备份与缓存清理语义。
//
// 真实挂载生产组件，假后端只替换 IPC 返回/拒绝：
// - 手动备份成功显示路径、版本与大小，并可从真实按钮在文件夹中显示；
// - 备份失败显示错误且不伪装成功；
// - 清理确认文案明确「只删已读/未收藏/未待同步」「未读不清理」「远端可能回灌」。

import assert from 'node:assert/strict';
import {
  componentTest, mount, flush, waitFor, resetStore, useAppStore,
} from './harness.mjs';
import {
  installBackend, clearInvokeCalls, setPlan, resetPlan, invokeCount, invokeCalls,
} from './fixtures.mjs';

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
  useAppStore.setState({ dataMode: 'tauri' });
  t.after(() => resetPlan());
  setPlan({ cache_cleanup: () => Promise.resolve('已清理 30 天前的已读文章 1 篇（未读/收藏/待同步保留）') });

  const { createElement, act } = await import('react');
  const { CacheCleanupSection } = await import('../../src/components/settings/CacheCleanupSection.tsx');
  const entry = await mount(createElement(CacheCleanupSection));
  await flush(2);

  const cleanup = findButton(entry.container, '清理旧文章');
  assert.ok(cleanup, '真实「清理旧文章」按钮存在');
  await act(async () => { cleanup.click(); });
  const dialog = document.body.querySelector('.confirm-overlay .confirm-dialog');
  assert.ok(dialog, '确认框真实挂载');
  const message = dialog.querySelector('.confirm-message')?.textContent ?? '';
  assert.ok(message.includes('已读、未收藏且无待同步状态'), `确认框说明清理条件: ${message}`);
  assert.ok(message.includes('未读文章不会清理'), `确认框说明未读保留: ${message}`);
  assert.ok(message.includes('远端仍有的内容可能在同步后重新导入'), `确认框说明远端回灌: ${message}`);
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
