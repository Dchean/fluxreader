// 一次性 CDP 直连工具（只用于验收探针，不进候选文件）
import { readFileSync } from 'node:fs';
import { connect, findPageTarget } from 'file:///D:/soft/fluxreader/tools/t059_cdp.mjs';

const port = Number(process.env.T059_CDP_PORT || 9224);
const target = await findPageTarget(port);
const cdp = await connect(target.webSocketDebuggerUrl);
const expr = readFileSync(process.argv[2], 'utf8');
const value = await cdp.evaluate(expr);
console.log(JSON.stringify(value, null, 1));
cdp.close();
