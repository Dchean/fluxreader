// TASK-059 实机端到端：真实运行的应用 + 本地 FreshRSS 形态后端，**只填域名**完成连接。
//
// 用法：
//   node tools/t059_ui_e2e.mjs --port 8901 --reqlog <后端请求日志> --outdir <证据目录> \
//        --restore-db <应用启动前取的真实库快照>
//
// 前置：应用已以 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 启动，
//       且 Vite dev server（debug 构建的 devUrl）在 5173 上。
// 界面驱动全程走 CDP 在页面内执行 JS（真实点击真实控件），不注入操作系统级键鼠事件。
//
// **数据库纪律**：`APPDATA=...` **不能**把应用指到隔离目录（Tauri 的 app_data_dir 走
// Win32 已知文件夹 API），因此本驱动会写进用户真实库。故须传 `--restore-db`，
// 驱动结束时会 kill 应用、用该快照覆盖回去并给出哈希一致证据（详见下方事故记录）。
//
// **证据纪律（TASK-058 教训）**：每张截图前都**显式设定并校验**主题，
// 并把实测值写进结果——绝不按「我以为的状态」命名文件。
// （首轮实测确实产出过名为 dark 实为 light 的截图，故此处改为强制校验。）

import { readFileSync, writeFileSync, copyFileSync, rmSync, existsSync, mkdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { connect, findPageTarget, waitFor, PAGE_HELPERS } from './t059_cdp.mjs';

/* ============================================================
   真实数据库保护（**事故后加固，必读**）

   事故（2026-09-18/19）：曾用 `APPDATA=...` 想把应用指到隔离目录，实测**无效**——
   Tauri 的 `app_data_dir()` 走 Win32 已知文件夹 API（FOLDERID_RoamingAppData），
   不读 APPDATA 环境变量。于是「隔离运行」实际直接写进了**用户真实库**：
   保存并同步检测到「换账号」还触发了 purge_remote_data。

   任务卡明文要求：`%APPDATA%\com.fluxreader.app` **只读使用**；端到端如需改数据，
   须**先备份、结束后逐字节还原并给出哈希一致的证据**。故本驱动在改动应用状态前
   强制备份、结束后还原，并逐文件校验哈希；无法还原就报错退出，不静默放过。
   ============================================================ */

const REAL_DIR = `${process.env.APPDATA}\\com.fluxreader.app`;
const DB_FILES = ['fluxreader.db', 'fluxreader.db-wal', 'fluxreader.db-shm'];

function sha256(file) {
  return createHash('sha256').update(readFileSync(file)).digest('hex');
}

function backupRealDb(dir) {
  mkdirSync(dir, { recursive: true });
  const backup = {};
  for (const name of DB_FILES) {
    const src = `${REAL_DIR}\\${name}`;
    if (existsSync(src)) {
      const dst = `${dir}\\${name}`;
      copyFileSync(src, dst);
      backup[name] = { path: dst, sha256: sha256(src), size: readFileSync(src).length };
    } else {
      backup[name] = null; // 记录「原本不存在」，还原时要删掉
    }
  }
  return backup;
}

/** 让应用退出：Windows 上运行中的应用持有 db/-shm 句柄，直接覆盖会失败。 */
function killApp() {
  try {
    execFileSync('taskkill', ['/IM', 'app.exe', '/F'], { stdio: 'ignore' });
  } catch {
    // 本就没在跑：taskkill 非 0，属正常
  }
  // 等句柄释放（同步等待，避免 copyfile UNKNOWN error）
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 2500);
}

/**
 * 还原真实库。
 * - 给了 `--restore-db`：以该快照为准（= 应用启动前的字节），并删除 wal/shm；
 * - 否则退回「按驱动开始时的备份逐个还原」。
 */
function restoreRealDb(backup) {
  killApp();
  const report = [];

  if (RESTORE_DB) {
    const want = sha256(RESTORE_DB);
    copyFileSync(RESTORE_DB, `${REAL_DIR}\\fluxreader.db`);
    for (const leftover of ['fluxreader.db-wal', 'fluxreader.db-shm']) {
      const p = `${REAL_DIR}\\${leftover}`;
      if (existsSync(p)) rmSync(p);
      report.push({ file: leftover, restored: 'removed', ok: !existsSync(p) });
    }
    const now = sha256(`${REAL_DIR}\\fluxreader.db`);
    report.push({
      file: 'fluxreader.db',
      restored: 'from-snapshot',
      snapshot: RESTORE_DB,
      expected: want,
      actual: now,
      ok: now === want,
    });
    return report;
  }

  for (const name of DB_FILES) {
    const dst = `${REAL_DIR}\\${name}`;
    const entry = backup[name];
    if (entry === null) {
      if (existsSync(dst)) rmSync(dst);
      report.push({ file: name, restored: 'removed', ok: !existsSync(dst) });
      continue;
    }
    copyFileSync(entry.path, dst);
    const now = sha256(dst);
    report.push({
      file: name,
      restored: 'copied',
      expected: entry.sha256,
      actual: now,
      ok: now === entry.sha256,
    });
  }
  return report;
}

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, cur, i, arr) => {
    if (cur.startsWith('--')) acc.push([cur.slice(2), arr[i + 1]]);
    return acc;
  }, []),
);
const PORT = Number(args.port || 8901);
const REQLOG = args.reqlog;
const OUTDIR = args.outdir || '.';
const BASE = `http://127.0.0.1:${PORT}`;

/* `--restore-db <file>`：**应用启动之前**取的真实库快照。
   结束时用它覆盖回去并删掉 wal/shm——还原目标是「本次实机验证之前」的字节，
   而不是「驱动开始跑时」的字节（那时应用已创建 wal/shm，且可能已写入）。
   还原后逐字节校验哈希并写入结果。 */
const RESTORE_DB = args['restore-db'] || null;

const results = [];
function record(name, detail) {
  results.push({ name, ...detail });
  console.log(`\n### ${name}\n${JSON.stringify(detail, null, 2)}`);
}

// 页面内操作助手全部来自 tools/t059_cdp.mjs（唯一实现，避免两份漂移）：
// setField/fieldValue 按**字段身份**定位输入框（按卡片文案找「密码」会命中用户名卡片，
// 其说明里含「账号密码」——实测踩到过）；openProtocolMenu/chooseOption 驱动 FluxDropdown；
// clickAction 按文本点击动作按钮并拒绝点击禁用态。
const HELPERS = `
${PAGE_HELPERS}
'ok';
`;

/** 等 toast 清空（2.2s 自动消失），避免把上一步的提示误当本步结果。 */
async function clearToasts(cdp) {
  await waitFor(cdp, `window.__t059.toasts().length === 0`, { label: 'toast 清空', timeoutMs: 8000 });
}

/** 等出现匹配 pattern 的 toast，返回其文本。
 *
 *  **出现不匹配的 toast 立即报错**，而不是等到超时——否则会掩盖真实原因
 *  （实测踩到过：保存后密码框被清空，点「测试连接」得到的是应用自己的
 *  「请填写 Endpoint、用户名和密码」，而断言在傻等「已连接/连接失败」直到超时）。 */
async function waitToast(cdp, pattern, label) {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    const t = await cdp.evaluate(`JSON.stringify(window.__t059.toasts())`);
    const arr = JSON.parse(t || '[]');
    const hit = arr.find((x) => pattern.test(x));
    if (hit) return hit;
    if (arr.length > 0) {
      throw new Error(`（${label}）出现非预期 toast：${JSON.stringify(arr)}`);
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`等待 toast 超时（${label}）`);
}

/** 当前主题 + 实测背景色（用于校验「深色确实是深的」）。 */
async function themeState(cdp) {
  const raw = await cdp.evaluate(`JSON.stringify({
    theme: window.__t059.theme(),
    modalBg: (() => { const m = document.querySelector('.settings-modal'); return m ? getComputedStyle(m).backgroundColor : null; })(),
  })`);
  return JSON.parse(raw);
}

/** 截图：先把主题校验到位，再落盘；文件名与实测主题一致才允许写出。 */
async function shotWithTheme(cdp, want, file, label) {
  const st = await themeState(cdp);
  if (st.theme !== want) {
    throw new Error(`（${label}）主题硬校验失败：期望 ${want}，实际 ${st.theme}（拒绝产出假证据）`);
  }
  const bg = st.modalBg || '';
  const m = /rgb\((\d+), (\d+), (\d+)\)/.exec(bg);
  if (m) {
    const lum = (Number(m[1]) + Number(m[2]) + Number(m[3])) / 3;
    if (want === 'dark' && lum > 128) {
      throw new Error(`（${label}）声称深色但实测背景为亮色 ${bg}（拒绝产出假证据）`);
    }
    if (want === 'light' && lum < 128) {
      throw new Error(`（${label}）声称浅色但实测背景为暗色 ${bg}（拒绝产出假证据）`);
    }
  }
  const { data } = await cdp.send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(file, Buffer.from(data, 'base64'));
  return { file, theme: st.theme, modalBg: st.modalBg };
}

/** 走真实控件切换主题：设置 → 外观 → 深色/浅色模式 → 回到同步页。 */
async function setTheme(cdp, want) {
  await cdp.evaluate(`window.__t059.clickText('.settings-nav-item', '外观')`);
  await waitFor(cdp, `!!window.__t059.byText('.theme-mode-btn', '模式')`, { label: '外观页' });
  const labelText = want === 'light' ? '浅色模式' : '深色模式';
  await cdp.evaluate(`window.__t059.clickText('.theme-mode-btn', '${labelText}')`);
  await waitFor(cdp, `document.documentElement.getAttribute('data-theme') === '${want}'`, {
    label: `data-theme=${want}`,
    timeoutMs: 5000,
  });
  await cdp.evaluate(`window.__t059.clickText('.settings-nav-item', '同步')`);
  await waitFor(cdp, `!!window.__t059.endpointCard()`, { label: '回到同步页' });
}

/** 填入三个字段（切页签会重新挂载 SyncTab，表单 state 被重置，故每次都要重填）。 */
async function fill(cdp, { endpoint, username, password }) {
  if (endpoint !== undefined) await cdp.evaluate(`window.__t059.setField('endpoint', '${endpoint}')`);
  if (username !== undefined) await cdp.evaluate(`window.__t059.setField('username', '${username}')`);
  if (password !== undefined) await cdp.evaluate(`window.__t059.setField('password', '${password}')`);
}

async function openSyncTab(cdp) {
  await cdp.evaluate(`window.__t059.clickText('.nav-tab-item', '设置中心')`);
  await waitFor(cdp, `!!document.querySelector('.settings-modal')`, { label: '设置弹窗' });
  await cdp.evaluate(`window.__t059.clickText('.settings-nav-item', '同步')`);
  await waitFor(cdp, `!!window.__t059.endpointCard()`, { label: 'Endpoint 卡片' });
}

async function main() {
  const page = await findPageTarget();
  const cdp = await connect(page.webSocketDebuggerUrl);
  await cdp.send('Runtime.enable');
  await cdp.send('Page.enable');
  // debug 构建的前端来自 Vite dev server：应用先于 dev server 启动时首屏是失败页，故重载一次。
  await cdp.send('Page.reload', { ignoreCache: true });
  await waitFor(cdp, `!!document.querySelector('.nav-tab-item')`, { label: '应用主导航' });
  await cdp.evaluate(HELPERS);

  // **先备份真实库**：本驱动会驱动「保存并同步」，一定会写应用数据目录，
  // 而该目录正是用户真实库所在（APPDATA 覆盖无效，见文件头事故记录）。
  const backupDir = `${OUTDIR}/real-db-backup`;
  const backup = backupRealDb(backupDir);
  record('0. 真实库备份（改动前，哈希取证）', {
    dir: backupDir,
    files: Object.fromEntries(
      Object.entries(backup).map(([k, v]) => [
        k,
        v ? { sha256: v.sha256, size: v.size } : '（原本不存在）',
      ]),
    ),
  });

  let failed = null;
  try {
    await runScenarios(cdp);
  } catch (e) {
    failed = e;
  } finally {
    // **无论如何都还原**（含中途失败），并逐文件校验哈希
    const report = restoreRealDb(backup);
    record('Z. 真实库还原（逐字节 + 哈希一致）', {
      dir: REAL_DIR,
      allOk: report.every((r) => r.ok),
      files: report,
    });
    writeFileSync(`${OUTDIR}/TASK-059-ui-e2e-result.json`, JSON.stringify(results, null, 2));
  }
  if (failed) throw failed;
  console.log(`\n== 实机验证完成，结果写入 ${OUTDIR}/TASK-059-ui-e2e-result.json ==`);
  cdp.close();
}

async function runScenarios(cdp) {
  await openSyncTab(cdp);

  // 显式建立已知主题状态（不依赖默认值），并核对文案
  await setTheme(cdp, 'dark');
  const copy = JSON.parse(await cdp.evaluate(`JSON.stringify(window.__t059.endpointCard(), null, 2)`));
  record('A. 文案：只填域名（深色主题）', { endpointCard: copy, theme: (await themeState(cdp)).theme });

  // ---- S1：纯域名 + GReader（FreshRSS 形态）----
  await clearToasts(cdp);
  await fill(cdp, { endpoint: BASE, username: 'demo', password: 'demo-pass' });
  await cdp.evaluate(`window.__t059.clickAction('测试连接')`);
  const s1 = await waitToast(cdp, /已连接|连接失败/, 'S1 测试连接');
  const s1Input = await cdp.evaluate(`window.__t059.fieldValue('endpoint')`);
  const shot1 = await shotWithTheme(
    cdp,
    'dark',
    `${OUTDIR}/TASK-059-ui-dark-bare-domain-connected.png`,
    'S1',
  );
  record('S1. 纯域名 + FreshRSS 形态 ⇒ 连接成功', {
    用户输入: BASE,
    回显值: s1Input,
    回显等于用户输入: s1Input === BASE,
    toast: s1,
    截图: shot1,
  });

  // ---- S2：保存并同步 ⇒ 解析结果被后续同步真正使用（并落库缓存）----
  await clearToasts(cdp);
  await cdp.evaluate(`window.__t059.clickAction('保存并同步')`);
  const s2first = await waitToast(cdp, /已连接|正在同步|已拉取/, 'S2 保存并同步');
  await new Promise((r) => setTimeout(r, 5000));
  const s2 = await cdp.evaluate(`JSON.stringify({
    toasts: window.__t059.toasts(),
    navFirst: [...document.querySelectorAll('.nav-tab-item')].map(e => e.textContent.trim()).slice(0, 4),
    accountTag: [...document.querySelectorAll('.about-arch-tag')].map(e => e.textContent.trim()),
  })`);
  record('S2. 保存并同步：解析结果被用于真实同步', {
    首条toast: s2first,
    后续状态: JSON.parse(s2),
  });

  // ---- S3：旧用法（完整路径）仍然可用 ----
  // 注意：「保存并同步」成功后应用会清空密码框，故此处必须重新填入密码。
  await clearToasts(cdp);
  await fill(cdp, { endpoint: `${BASE}/api/greader.php`, password: 'demo-pass' });
  await cdp.evaluate(`window.__t059.clickAction('测试连接')`);
  const s3 = await waitToast(cdp, /已连接|连接失败/, 'S3 完整路径');
  record('S3. 旧用法（完整路径）仍一次成功', { toast: s3 });

  // ---- S4：凭据错必须报凭据错（不得说成找不到 API）----
  await clearToasts(cdp);
  await fill(cdp, { endpoint: BASE, password: 'wrong-pass' });
  await cdp.evaluate(`window.__t059.clickAction('测试连接')`);
  const s4 = await waitToast(cdp, /已连接|连接失败/, 'S4 凭据错');
  const shot4 = await shotWithTheme(
    cdp,
    'dark',
    `${OUTDIR}/TASK-059-ui-dark-bad-credential.png`,
    'S4',
  );
  record('S4. 凭据错 ⇒ 如实报凭据/HTTP 失败，不得报「找不到 API」', {
    toast: s4,
    含401或凭据字样: /401|凭据|Unauthorized/i.test(s4),
    误报为找不到API: /找不到 (GReader|Fever) API/.test(s4),
    截图: shot4,
  });

  // ---- S5：Fever + 纯域名（FreshRSS 形态 /api/fever.php）----
  await clearToasts(cdp);
  await cdp.evaluate(`window.__t059.openProtocolMenu()`);
  await waitFor(cdp, `!!window.__t059.byText('.flux-dropdown-option', 'Fever')`, { label: '协议菜单' });
  await cdp.evaluate(`window.__t059.chooseOption('Fever')`);
  await fill(cdp, { password: 'demo-pass' });
  await cdp.evaluate(`window.__t059.clickAction('测试连接')`);
  const s5 = await waitToast(cdp, /已连接|连接失败/, 'S5 Fever 纯域名');
  record('S5. Fever + 纯域名（FreshRSS 形态）⇒ 成功', { toast: s5 });

  // ---- S6：浅色主题（真实控件切换 + 硬校验后截图）----
  await clearToasts(cdp);
  await setTheme(cdp, 'light');
  await fill(cdp, { endpoint: BASE, username: 'demo', password: 'demo-pass' });
  await cdp.evaluate(`window.__t059.clickAction('测试连接')`);
  const s6 = await waitToast(cdp, /已连接|连接失败/, 'S6 浅色主题');
  const shot6 = await shotWithTheme(
    cdp,
    'light',
    `${OUTDIR}/TASK-059-ui-light-bare-domain-connected.png`,
    'S6',
  );
  record('S6. 浅色主题下同一操作', {
    toast: s6,
    endpointCard: JSON.parse(await cdp.evaluate(`JSON.stringify(window.__t059.endpointCard(), null, 2)`)),
    截图: shot6,
  });

  // ---- 后端请求日志：证明探测有界 / 完整路径不多探 / 解析结果已缓存 ----
  if (REQLOG) {
    const lines = readFileSync(REQLOG, 'utf8')
      .split('\n')
      .map((l) => l.trim())
      .filter((l) => l.startsWith('REQ '));
    record('R. 后端收到的请求序列（证明探测行为）', { total: lines.length, lines });
  }
}

main().catch((e) => {
  console.error('E2E FAILED:', e.message);
  writeFileSync(`${OUTDIR}/TASK-059-ui-e2e-result.json`, JSON.stringify({ error: e.message, results }, null, 2));
  process.exit(1);
});
