// OPT-016B：源码态契约的条件变异取证（只 tmp/同名副本，生产源码零改动）。
//
// 为什么需要：DOM 断言必须被证明「能判红」——否则「危险标签未创建」可能只是
// 断言写错了。这里把 src/components/ReaderProse.tsx 复制到 tmp/，只做一个条件
// 变异（源码态文本插值 → dangerouslySetInnerHTML），再用**与主用例完全相同**的
// sourceModeChecks 断言它必须失败。变异副本用后即删；测试末尾复核生产源码文件
// 字节未变（「不改生产留变异」）。

import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdirSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { componentTest, mount, flush, unmount, resetStore } from './harness.mjs';
import { sourceModeChecks } from './reader-prose-checks.mjs';

const PROD_PATH = fileURLToPath(new URL('../../src/components/ReaderProse.tsx', import.meta.url));
const TMP_DIR = fileURLToPath(new URL('../../tmp/opt016b-mutation/', import.meta.url));

const TEST_HTML = [
  '<p class="safe-p">安全段落</p>',
  '<script>window.__XSS_SCRIPT__=1</script>',
  '<img src="https://images.example.com/p.png" onerror="window.__XSS_ERR__=1" alt="p">',
  '<a href="https://example.com/link">链接</a>',
].join('');

componentTest('OPT-016B 变异取证：源码态改回 dangerouslySetInnerHTML 必须被同一组 DOM 断言判红', { timeout: 30000 }, async () => {
  const src = readFileSync(PROD_PATH, 'utf8');
  const mutated = src.replace(
    '<pre className="reader-source-view">{sourceText}</pre>',
    '<pre className="reader-source-view" dangerouslySetInnerHTML={{ __html: sourceText }} />',
  );
  assert.notEqual(mutated, src, '变异必须真实命中 ReaderProse 的源码态文本插值（否则本用例无意义）');

  mkdirSync(TMP_DIR, { recursive: true });
  const tmpFile = join(TMP_DIR, `ReaderProse.mutated-${process.pid}.tsx`);
  writeFileSync(tmpFile, mutated, 'utf8');

  resetStore();
  let entry = null;
  try {
    const { ReaderProse } = await import(pathToFileURL(tmpFile).href);
    const { createElement } = await import('react');
    entry = await mount(createElement(ReaderProse, {
      renderHtml: TEST_HTML,
      sourceText: TEST_HTML,
      isSourceMode: true,
      isStreamingTranslation: false,
      style: { fontFamily: 'serif', fontSize: 16, lineHeight: 1.8 },
      onClick: () => {},
    }));
    await flush(1);

    const checks = sourceModeChecks(entry.container, TEST_HTML);
    const failed = checks.filter((c) => !c.pass);
    assert.ok(failed.length >= 3,
      `变异副本必须被同一组断言判红（至少 script/img/a 三条），实际判红 ${failed.length} 条：`
      + JSON.stringify(failed.map((f) => f.name)));

    // 如实记录抓住了变异的检查项（RESULT 引用，不是推断）。
    console.log(`[mutation] 判红检查项：${failed.map((f) => f.name).join(' | ')}`);
    assert.ok(failed.some((f) => f.name.includes('<script>')), '「<script> 未创建」必须判红');
    assert.ok(failed.some((f) => f.name.includes('<img>')), '「<img> 未创建」必须判红');
  } finally {
    if (entry) await unmount(entry);
    rmSync(tmpFile, { force: true });
  }

  // 生产源码零改动（防「变异留在生产」）。
  assert.equal(readFileSync(PROD_PATH, 'utf8'), src, '生产 ReaderProse.tsx 必须与变异前逐字节一致');
});
