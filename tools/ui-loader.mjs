// Node ESM loader（配合 tools/frontend-regression.mjs 的组件侧证据）：
// 把仓库里的 .tsx / .tsx 源码用 rolldown（已在 devDependencies，Vite 的打包器）
// 就地转译成 ESM，使 Node 能直接 import React 组件模块。
//
// 为什么需要：先前组件侧没有可执行的验证手段（`npm run test:frontend` 只编译
// src/store.ts 那一支、`node --loader ./tools/test-loader.mjs` 只驱动 store）。
//
// 本文件是**唯一**一份实现：frontend-regression.mjs 通过
// `register(new URL('./ui-loader.mjs', import.meta.url))` 直接引用它，没有内联复刻、
// 也不存在「两份字节一致」的断言（早先版本的本注释曾如此声称，属笔误，已订正）。
// 因此改动本文件会同时作用于组件侧断言，无需同步第二处。
//
// 不引入新依赖：rolldown 来自 Vite 的依赖树；.tsx 由 rolldown 的 oxc 转译器
// 生成 `react/jsx-dev-runtime` 的 jsx()/jsxs() 调用（与 Vite 同一条链路）。

import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';

/** 把 node_modules 里的 react/jsx-runtime 指到实际的 .js 文件：
 *  Node 的 exports 映射对 "react/jsx-runtime" 只给了 "react/jsx-runtime.js"，
 *  这里显式解析避免子路径解析差异。 */
const JSX_SHIMS = new Map([
  ['react/jsx-dev-runtime', 'react/jsx-dev-runtime.js'],
  ['react/jsx-runtime', 'react/jsx-runtime.js'],
]);

/** 需要就地转译的源码扩展名。只处理仓库 src/ 下的 TS 源码——
 *  dist-test/ 下已是 tsc 产物（.js），不能再被本 loader 接管，
 *  否则「省略扩展名」的兜底会把 dist-test 的 './store.js' 抢走。 */
const TRANSPILE_EXT = ['.tsx', '.jsx', '.ts'];
const SRC_DIR = fileURLToPath(new URL('../src/', import.meta.url));

/** src/store.ts → dist-test/store.js 的别名表。
 *
 *  组件只依赖 store 的**同一个实例**；而 dist-test 是 tsc 的产物、src 是源码。
 *  若让组件链自己 import src/store.ts，会与 harness 已导入的 dist-test/store.js
 *  形成两个互相独立的 zustand store（组件读到的状态与断言读到的不是一份），
 *  断言就失去意义。这里把 src/store.ts 统一重定向到 dist-test/store.js，
 *  保证「组件看到的就是被断言的那份状态」。 */
const STORE_ALIAS = new Map([[`${SRC_DIR}store.ts`, 'dist-test/store.js']]);

let rolldownPromise = null;
async function getRolldown() {
  if (!rolldownPromise) rolldownPromise = import('rolldown');
  return rolldownPromise;
}

/** 用 rolldown 单文件转译（external 全开，不做打包，只做语法降级）。 */
async function transpile(filename) {
  const { rolldown } = await getRolldown();
  const bundle = await rolldown({ input: filename, platform: 'neutral', external: () => true });
  const { output } = await bundle.generate({ format: 'esm' });
  await bundle.close?.();
  return output[0].code;
}

export async function resolve(specifier, context, nextResolve) {
  const shim = JSX_SHIMS.get(specifier);
  if (shim) {
    /* 从调用方的位置解析，保证无论从哪个 cwd 启动都指向同一份 react */
    const parent = context.parentURL ? fileURLToPath(context.parentURL) : `${process.cwd()}/probe.js`;
    const cut = parent.search(/[\\/]node_modules[\\/]/);
    const root = cut >= 0 ? parent.slice(0, cut + 1) : `${process.cwd()}/`;
    return { url: pathToFileURL(`${root}node_modules/${shim}`).href, shortCircuit: true };
  }
  /* src/ 下的「省略扩展名」必须在 nextResolve 之前处理：链上的
     tools/test-loader.mjs 会给任何无扩展名相对路径盲加 .js（它是为 dist-test
     产物写的），若不抢先，src/store.ts 会被解析成不存在的 src/store.js。 */
  if (specifier.startsWith('.') && context.parentURL) {
    const parent = fileURLToPath(context.parentURL);
    if (parent.startsWith(SRC_DIR)) {
      const direct = await tryExtensions(specifier, context);
      if (direct) {
        const aliased = STORE_ALIAS.get(fileURLToPath(direct.url));
        if (aliased) return { url: pathToFileURL(aliased).href, shortCircuit: true };
        return direct;
      }
    }
  }
  return nextResolve(specifier, context);
}

/** 依次试探 bundler 风格的候选（.ts/.tsx/index.*），命中即返回。 */
async function tryExtensions(specifier, context) {
  for (const cand of [`${specifier}.ts`, `${specifier}.tsx`, `${specifier}/index.ts`, `${specifier}/index.tsx`]) {
    const url = new URL(cand, context.parentURL).href;
    if (existsSync(fileURLToPath(url))) return { url, shortCircuit: true };
  }
  return null;
}

export async function load(url, context, nextLoad) {
  if (url.startsWith('file:') && TRANSPILE_EXT.some((ext) => url.endsWith(ext))) {
    return { format: 'module', source: await transpile(fileURLToPath(url)), shortCircuit: true };
  }
  return nextLoad(url, context);
}

/** 供回归网做「两份 loader 源码一致」的字节校验。 */
export const UI_LOADER_PATH = fileURLToPath(import.meta.url);

/** 读回本文件源码（harness 用来比对内联副本）。 */
export async function readOwnSource() {
  return readFile(import.meta.url, 'utf8');
}
