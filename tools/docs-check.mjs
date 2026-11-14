// 文档结构门禁：索引覆盖、相对链接、锚点、行号禁令。
// 只查结构，不判内容对错；内容与代码一致性靠 docs/maintenance.md 第 4 节的流程。
import { readdirSync, readFileSync, existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const DOCS = join(ROOT, 'docs');
const INDEX = join(DOCS, 'README.md');
const LINE_REF = /\b[\w./-]+\.(?:rs|ts|tsx|js|mjs|css|sql|json|md|yml|yaml):\d+/g;
const SCHEME = /^[a-z][a-z0-9+.-]*:/i;
const FENCE = /^\s*(?:`{3}|~{3})/;

const failures = [];
const fail = (file, msg) => failures.push(file + ': ' + msg);
const isExternal = (t) => SCHEME.test(t) || t.startsWith('//');

/** 逐行去掉围栏代码块，避免把示例里的方括号当链接。 */
const stripFences = (text) => {
  const out = [];
  let fenced = false;
  for (const line of text.split('\n')) {
    if (FENCE.test(line)) { fenced = !fenced; continue; }
    if (!fenced) out.push(line);
  }
  return out.join('\n');
};

/** 提取行内链接目标：扫描 ]( 到下一个 )，避免正则转义的歧义。 */
const linkTargets = (text) => {
  const out = [];
  let i = 0;
  while ((i = text.indexOf('](', i)) !== -1) {
    const start = i + 2;
    const end = text.indexOf(')', start);
    if (end === -1) break;
    const raw = text.slice(start, end).trim().split(' ')[0];
    if (raw) out.push(raw);
    i = end + 1;
  }
  return out;
};

const slug = (heading) =>
  heading
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s_-]/gu, '')
    .replace(/\s+/g, '-');

/** 目标文档可用的锚点集合：显式 a id 与标题锚。 */
const anchorsOf = (text) => {
  const set = new Set();
  for (const m of text.matchAll(/<a\s+id="([^"]+)"/g)) set.add(m[1].toLowerCase());
  for (const m of text.matchAll(/^#{1,6}\s+(.+)$/gm)) set.add(slug(m[1]));
  return set;
};

const files = readdirSync(DOCS).filter((f) => f.endsWith('.md'));
const indexText = readFileSync(INDEX, 'utf8');

// 1) 索引覆盖
for (const f of files) {
  if (f === 'README.md') continue;
  if (!indexText.includes('](' + f + ')')) fail('docs/README.md', '文档未登记在索引：' + f);
}

// 1b) 文档待办清单必须存在（规则要求登记缺口的地方）
const ROADMAP = join(DOCS, 'roadmap.md');
if (existsSync(ROADMAP) && !readFileSync(ROADMAP, 'utf8').includes('## 文档待办')) {
  fail('docs/roadmap.md', '缺少「文档待办」节：文档缺口必须登记在这里');
}

// 2) 链接解析 + 3) 锚点存在
for (const f of files) {
  const full = join(DOCS, f);
  const text = stripFences(readFileSync(full, 'utf8'));
  for (const target of linkTargets(text)) {
    if (isExternal(target)) continue;
    const hash = target.indexOf('#');
    const pathPart = decodeURIComponent(hash === -1 ? target : target.slice(0, hash));
    const frag = hash === -1 ? '' : decodeURIComponent(target.slice(hash + 1));
    const abs = pathPart ? resolve(dirname(full), pathPart) : full;
    if (!existsSync(abs)) { fail('docs/' + f, '链接目标不存在：' + target); continue; }
    if (!frag || !abs.endsWith('.md')) continue;
    if (!anchorsOf(readFileSync(abs, 'utf8')).has(frag.toLowerCase())) {
      fail('docs/' + f, '锚点不存在：' + target);
    }
  }
}

// 4) 行号禁令
for (const f of files) {
  for (const m of readFileSync(join(DOCS, f), 'utf8').matchAll(LINE_REF)) {
    fail('docs/' + f, '不得写行号引用（改用文件 + 符号名）：' + m[0]);
  }
}

if (failures.length) {
  console.error('[docs-check] 失败 ' + failures.length + ' 项：');
  for (const f of failures) console.error('  - ' + f);
  process.exit(1);
}
console.log('[docs-check] 通过：' + files.length + ' 篇文档，索引覆盖、链接与锚点、行号禁令均无问题。');
