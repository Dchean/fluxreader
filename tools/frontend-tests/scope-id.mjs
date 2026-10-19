// tools/frontend-tests/scope-id.mjs
// 领域模块：字符串 id → 后端数字 id 解析的单点收敛 —— OPT-017F。
//
// 证据分层（先红后绿口径）：
// - 行为真值表：numericId 对全部 scope 形态（all/分类/源/纯数字/非法/负数/空串）
//   的解析结果逐条钉死（收敛不改行为）；
// - 消费边界字段映射：internals 的范围派生（QueryScope.markScope）对每个形态
//   的 feed_id / folder_id 用**独立期望表**断言（含另一侧必须为 null）——锁字段
//   路由与 NaN/null 语义，不再以 numericId 作为期望源（收敛后两处同源，直接
//   比较会退化为同义反复）；
// - 同形复制源级守卫（断言纪律允许的「单点性必要」场景）：src 下恰有一处
//   `match(/(\d+)/)` 字面实现、`numericId` 的 function/const/let/var 定义恰一份，
//   定义在 store/scopeId.ts；selectors 只 re-export、internals 只 import。
//   口径边界：改换写法的语义级副本（如 `parseInt`、无捕获组正则）**不在**源级
//   守卫覆盖内，需评审发现——断言文案不宣称「任何重复实现回潮即红」。
//
// 红证据口径（如实说明）：本域在收敛前运行时，(s0)/(s3)/(s4) 因共享模块尚不存在、
// 两份定义并存而判红（动态导入兜底为 null，不产生 TypeError 中断）；行为真值表与
// 消费边界在旧实现（两份逐字相同副本）上本就是绿——「先断言两实现同结果再收敛」
// 成立的形态。行为断言的判别力由 RESULT 记录的变异 M1（临时改解析实现）证明。
export const id = 'scope-id';

export async function run(ctx) {
  const { nSel, checkNew } = ctx;

  /* 共享模块（收敛后存在；旧实现下不存在 → null 兜底，红为逐条断言而非崩溃） */
  let scopeIdMod = null;
  try {
    scopeIdMod = await import('../../dist-test/store/scopeId.js');
  } catch {
    scopeIdMod = null;
  }
  const internals = await import('../../dist-test/store/internals.js');
  const num = nSel.numericId;
  const sameNum = (a, b) => Object.is(a, b); // Object.is(NaN, NaN) === true

  /* ============================================================
     (s0) 单点实现存在（共享叶模块）
     ============================================================ */
  checkNew('(s0) 共享模块 store/scopeId.ts 导出唯一实现 numericId（selectors 与 internals 的共同来源）',
    typeof scopeIdMod?.numericId === 'function');

  /* ============================================================
     (s1) 行为真值表：全部 scope 形态（收敛不改行为）
     ============================================================ */
  /* 形态选择口径：all/category/feed/非法/负数/空串（卡内强制反例）
     + 既有语义边界（前缀残缺、混合串、空白）。语义 = 取第一段连续数字，
     无数字段 → NaN。 */
  const feedNum = [['12', 12], ['feed-12', 12], ['x9y8', 9], [' 42 ', 42]];
  const catNum = [['cat-7', 7], ['cat-12x', 12]];
  const invalid = [['all', NaN], ['abc', NaN], ['cat-', NaN], ['feed-', NaN], ['', NaN]];
  const negative = [['-5', 5]]; // 第一段数字语义：负号不参与解析（既有行为，删除负号即改行为）
  const feedBad = feedNum.filter(([s, v]) => !sameNum(num(s), v)).map(([s]) => s);
  const catBad = catNum.filter(([s, v]) => !sameNum(num(s), v)).map(([s]) => s);
  const invalidBad = invalid.filter(([s, v]) => !sameNum(num(s), v)).map(([s]) => s);
  const negativeBad = negative.filter(([s, v]) => !sameNum(num(s), v)).map(([s]) => s);
  checkNew('(s1) 纯数字/feed 前缀形态：取第一段连续数字（12→12、feed-12→12、x9y8→9、空白包裹→42）',
    typeof num === 'function' && feedBad.length === 0, feedBad.join('；'));
  checkNew('(s1) 分类前缀形态：cat-7→7、cat-12x→12（第一段数字，不按定长 slice 截断）',
    catBad.length === 0, catBad.join('；'));
  checkNew('(s1) 非法形态（all/abc/前缀残缺/空串）→ NaN（不猜、不落成 0）',
    invalidBad.length === 0, invalidBad.join('；'));
  checkNew('(s1) 负数形态：-5→5（第一段连续数字语义；等值收敛保持既有行为，不引入取整/取绝对值）',
    negativeBad.length === 0, negativeBad.join('；'));

  /* ============================================================
     (s2) 消费边界字段映射：QueryScope.markScope 对每个形态的
     feed_id / folder_id 期望（独立表；两个字段都断言，含另一侧必须为 null）
     ============================================================ */
  const scopeExpect = [
    ['all', null, null],
    ['cat-7', null, 7],
    ['cat-12x', null, 12],
    ['12', 12, null],
    ['feed-12', 12, null],
    ['x9y8', 9, null],
    [' 42 ', 42, null],
    ['abc', NaN, null],
    ['cat-', null, NaN],
    ['feed-', NaN, null],
    ['', NaN, null],
    ['-5', 5, null],
  ];
  const consumerBad = [];
  for (const [s, feedId, folderId] of scopeExpect) {
    const m = internals.QueryScope.markScope(s);
    if (!sameNum(m.feed_id, feedId) || !sameNum(m.folder_id, folderId)) {
      consumerBad.push(`${JSON.stringify(s)}:(${String(m.feed_id)},${String(m.folder_id)})!=(${String(feedId)},${String(folderId)})`);
    }
  }
  checkNew('(s2) 消费边界字段映射：QueryScope.markScope 对全部形态的 feed_id / folder_id 与独立期望表一致（另一侧必须为 null；非法形态保持 NaN）',
    consumerBad.length === 0, consumerBad.join('；'));

  /* ============================================================
     (s3) 公共表面保留：store.js 的 numericId（selectors re-export 链）与
     单点实现是同一个函数引用（slices/feeds 的既有 import 路径不变）
     ============================================================ */
  checkNew('(s3) 公共导出链保留：store.js（selectors 面）的 numericId 与 store/scopeId 的实现同引用，且可调用',
    typeof num === 'function' && scopeIdMod !== null && num === scopeIdMod.numericId);

  /* ============================================================
     (s4) 同形复制源级守卫（防同一写法/定义形态的副本回潮）
     ============================================================ */
  const fs = await import('node:fs');
  const path = await import('node:path');
  const url = await import('node:url');
  const srcDir = url.fileURLToPath(new URL('../../src/', import.meta.url));
  const walk = (dir) => fs.readdirSync(dir, { withFileTypes: true }).flatMap((d) => {
    const p = path.join(dir, d.name);
    return d.isDirectory() ? walk(p) : (/\.(ts|tsx)$/.test(d.name) ? [p] : []);
  });
  const srcFiles = walk(srcDir).map((file) => ({ file, src: fs.readFileSync(file, 'utf8') }));
  const parseSites = [];
  const defs = [];
  /* 定义形态：function 声明与 const/let/var 赋值（含 export 前缀）。
     语义级副本（parseInt / 无捕获组正则 / 改名函数）不在此守卫范围。 */
  const defPatterns = [
    [/function\s+numericId\s*\(/g, 'function numericId('],
    [/(?:const|let|var)\s+numericId\s*=/g, 'const/let/var numericId ='],
    [/function\s+scopeNumericId\s*\(/g, 'function scopeNumericId('],
    [/(?:const|let|var)\s+scopeNumericId\s*=/g, 'const/let/var scopeNumericId ='],
  ];
  for (const { file, src } of srcFiles) {
    const rel = path.relative(srcDir, file).replaceAll('\\', '/');
    if (/match\(\/\(\\d\+\)\/\)/.test(src)) parseSites.push(rel);
    for (const [pattern, label] of defPatterns) {
      const n = (src.match(pattern) ?? []).length;
      if (n) defs.push(`${rel}:${label}×${n}`);
    }
  }
  checkNew('(s4) 同形复制守卫：src 下恰一处 `match(/(\\d+)/)` 字面实现，且位于 store/scopeId.ts（同形复制回潮即红；改名/换写法的语义级副本不在本守卫覆盖内）',
    parseSites.length === 1 && parseSites[0] === 'store/scopeId.ts', parseSites.join(','));
  checkNew('(s4) 同形定义守卫：numericId 的 function/const/let/var 定义恰一份（store/scopeId.ts）；scopeNumericId 的两种定义形态零份',
    defs.length === 1 && defs[0] === 'store/scopeId.ts:function numericId(×1', defs.join('；'));
  const selectorsSrc = srcFiles.find((f) => f.file.endsWith(path.join('store', 'selectors.ts')))?.src ?? '';
  const internalsSrc = srcFiles.find((f) => f.file.endsWith(path.join('store', 'internals.ts')))?.src ?? '';
  checkNew('(s4) 单点消费：selectors 只 re-export、internals 只 import 共享模块（两处都不再自带解析实现）',
    selectorsSrc.includes("from './scopeId'") && internalsSrc.includes("from './scopeId'")
    && !/match\(\/\(\\d\+\)\/\)/.test(selectorsSrc) && !/match\(\/\(\\d\+\)\/\)/.test(internalsSrc));
}
