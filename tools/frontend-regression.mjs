// Note: 回归网只锁行为，源码文本断言仅留「单点性必要」 — 见 .agents/notes/implemented/testing/2026-09-16-测试门禁与断言纪律.md
// 前端逻辑回归（无浏览器）编排入口（OPT-016C 拆分）：创建共享 harness → 按明确顺序 await 域模块
// （实现见 tools/frontend-tests/，按功能而非历史 TASK 号命名）→ 统一汇总/非 0 退出。
// 运行：先 npx tsc -p tsconfig.test.json，再
//   node --loader ./tools/test-loader.mjs ./tools/frontend-regression.mjs
// 单域独立运行（域入口先 await useMainBackend()，各域自带复位、无跨域状态前置）：
//   node --loader ./tools/test-loader.mjs ./tools/frontend-regression.mjs --domain ui-contracts
//   （可重复 --domain；缺省 = 全部域按下方顺序执行）
import { createHarness } from './frontend-tests/harness.mjs';
import * as d0 from './frontend-tests/smoke-baseline.mjs';
import * as d1 from './frontend-tests/bootstrap-and-query.mjs';
import * as d2 from './frontend-tests/timeline-and-playback.mjs';
import * as d3 from './frontend-tests/reader-and-ai.mjs';
import * as d4 from './frontend-tests/mutations-and-counts.mjs';
import * as d5 from './frontend-tests/settings-and-feeds.mjs';
import * as d6 from './frontend-tests/pagination-and-cache.mjs';
import * as d7 from './frontend-tests/scroll-position-restore.mjs';
import * as d8 from './frontend-tests/ui-contracts.mjs';

const DOMAINS = [d0, d1, d2, d3, d4, d5, d6, d7, d8];

const argv = process.argv.slice(2);
const selected = [];
for (let i = 0; i < argv.length; i += 1) {
  const a = argv[i];
  if (a === '--domain') {
    if (!argv[i + 1]) {
      console.error('--domain 需要一个域名称');
      process.exit(2);
    }
    selected.push(argv[i + 1]);
    i += 1;
  } else if (a.startsWith('--domain=')) {
    selected.push(a.slice('--domain='.length));
  } else if (a === '--list-domains') {
    console.log(DOMAINS.map((d) => d.id).join('\n'));
    process.exit(0);
  } else {
    console.error('未知参数: ' + a);
    console.error('可用域: ' + DOMAINS.map((d) => d.id).join(', '));
    process.exit(2);
  }
}
const unknown = selected.filter((s) => !DOMAINS.some((d) => d.id === s));
if (unknown.length) {
  console.error('未知域: ' + unknown.join(', '));
  console.error('可用域: ' + DOMAINS.map((d) => d.id).join(', '));
  process.exit(2);
}
const runList = selected.length ? DOMAINS.filter((d) => selected.includes(d.id)) : DOMAINS;

const ctx = createHarness();
for (const domain of runList) {
  console.log('\n--- [domain: ' + domain.id + '] ---');
  ctx.beginDomain(domain.id);
  await domain.run(ctx);
}

const failed = ctx.results.filter((r) => !r.pass);
const newFailed = ctx.newResults.filter((r) => !r.pass);
const totalAll = ctx.results.length + ctx.newResults.length;
const totalFailed = failed.length + newFailed.length;
console.log('\n=== 既有回归 ' + (ctx.results.length - failed.length) + '/' + ctx.results.length + ' 通过（TASK-122 真值源迁移：S-1/S-2/S-4 读取面按新架构更新，保护意图逐字保留，理由见各处改动注） ===');
console.log('=== 新增 store 行为断言 ' + (ctx.newResults.length - newFailed.length) + '/' + ctx.newResults.length + ' 通过 🆕 ===');
console.log('=== 前端逻辑回归合计 ' + (totalAll - totalFailed) + '/' + totalAll + ' 通过 ===');
for (const domain of DOMAINS) {
  const own = [...ctx.results, ...ctx.newResults].filter((r) => r.domain === domain.id);
  if (!own.length) continue;
  const ownFailed = own.filter((r) => !r.pass).length;
  console.log('  [domain: ' + domain.id + '] ' + (own.length - ownFailed) + '/' + own.length + (ownFailed ? ' ❌' : ''));
}
if (totalFailed) {
  if (failed.length) console.error('既有失败项:', failed.map((f) => f.name).join('; '));
  if (newFailed.length) console.error('新增失败项:', newFailed.map((f) => f.name).join('; '));
  process.exit(1);
}
process.exit(0);
