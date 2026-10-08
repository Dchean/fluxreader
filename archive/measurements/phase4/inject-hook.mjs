// 一次性：注入页面内测量库 + R1 验收钩子（只用于验收探针，不进候选文件）
import { readFileSync } from 'node:fs';
import { connect, findPageTarget } from 'file:///D:/soft/fluxreader/tools/t059_cdp.mjs';

const src = readFileSync('D:/soft/fluxreader/tools/phase4_measure.mjs', 'utf8');
const start = src.indexOf('const PAGE_LIB = `');
const end = src.indexOf('`;', start);
const pageLib = new Function('return ' + src.slice(start + 'const PAGE_LIB = '.length, end + 1) + ';')();

const target = await findPageTarget(Number(process.env.T059_CDP_PORT || 9224));
const cdp = await connect(target.webSocketDebuggerUrl);
console.log('lib:', await cdp.evaluate(pageLib));
console.log('hook:', JSON.stringify(await cdp.evaluate(readFileSync(process.argv[2], 'utf8'))));
cdp.close();
