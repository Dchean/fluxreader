// Node ESM loader：mock @tauri-apps/api/core 与 @tauri-apps/plugin-opener，
// 并把相对路径无扩展名 import 解析为 .js（tsc 产物）。
// 用法：node --loader ./tools/test-loader.mjs ./tools/frontend-regression.mjs
//
// invoke 通过 globalThis.__INVOKE__ 注入（测试驱动脚本先设好再 import store）。

import { pathToFileURL } from 'node:url';
import { fileURLToPath } from 'node:url';
import { dirname, resolve as resolvePath, extname } from 'node:path';

const CORE_MOCK = `
export const invoke = (cmd, args) => globalThis.__INVOKE__(cmd, args);
export class Channel {
  constructor(onmessage){ this._m = onmessage ?? null; }
  set onmessage(h){ this._m = h; }
  get onmessage(){ return this._m; }
}
export const isTauri = () => true;
`;

const OPENER_MOCK = `export const openUrl = () => Promise.resolve();`;

const MOCKS = {
  '@tauri-apps/api/core': CORE_MOCK,
  '@tauri-apps/plugin-opener': OPENER_MOCK,
};

export async function resolve(specifier, context, nextResolve) {
  if (specifier in MOCKS) {
    return { url: 'mock:' + specifier, shortCircuit: true };
  }
  if (specifier.startsWith('./') || specifier.startsWith('../')) {
    const parentPath = context.parentURL ? fileURLToPath(context.parentURL) : process.cwd();
    let resolved = resolvePath(dirname(parentPath), specifier);
    if (!extname(resolved)) resolved += '.js';
    return { url: pathToFileURL(resolved).href, shortCircuit: true };
  }
  return nextResolve(specifier, context);
}

export async function load(url, context, nextLoad) {
  if (url.startsWith('mock:')) {
    const key = url.slice('mock:'.length);
    return { format: 'module', source: MOCKS[key], shortCircuit: true };
  }
  return nextLoad(url, context);
}
