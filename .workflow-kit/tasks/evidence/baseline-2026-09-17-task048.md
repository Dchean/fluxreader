# TASK-048 基线（2026-09-17）：前端行为测试扩充前

采集于 TASK-047 验收之后（提交 `f0476b0`），工作区干净。

## 为什么需要本任务

BRIEF 的重构评估原文：

> 前端 7.8k 行：`SettingsModal.tsx` 1512 行、`store.ts` 1493 行为最大单体；
> **无组件级行为测试，仅类型检查式回归**
>
> 取舍：总量较大但每步有验证点；**前端需先补行为测试，起步成本高于后端**

owner 于 2026-09-17 选择「先补 store 行为测试，再拆」（`DEC-a2c6584f8d4442daa3a65eb0bcc7ddaf`）。

## 前端拆分目标现状（可信口径）

行数用可信口径：LF 字节数 = Python `splitlines()` = .NET `ReadAllLines()`。

| 文件 | 行数 |
| --- | --- |
| `src/store.ts` | 1657 |
| `src/components/SettingsModal.tsx` | 1513 |
| `src/components/Overlays.tsx` | 755 |
| `src/components/Timeline.tsx` | 733 |
| `src/lib/api.ts` | 579 |

## 既有前端测试机制（本任务沿用，不引入新框架）

`package.json`：

```
test:frontend = tsc -p tsconfig.test.json && node --loader ./tools/test-loader.mjs ./tools/frontend-regression.mjs
```

- TS 编译到 `dist-test/`，测试文件 `await import('../dist-test/store.js')` 直接驱动 store；
- 断言用文件内的 `check(name, cond)` 收集；
- 纯状态机、无 DOM、无网络、无 Tauri。

## 基线结果（本会话实跑）

```
npm run lint          => 0 warnings and 0 errors
npm run build         => ✓ built（含 tsc）
npm run test:frontend => 26/26 通过，退出码 0
cargo test            => 120 passed / 0 failed / 23 ignored（本任务不改 Rust）
```

## 既有 26 项断言的覆盖盲区（本任务要补的部分）

对 `tools/frontend-regression.mjs` 做 grep：

```
focus|querySelector|document\.|getComputedStyle|classList|tabIndex|role=   => 命中 0
```

即它只覆盖少数状态流转（bootstrap、翻译回读、CSP 消毒、toast 重试等），
**不覆盖**：订阅/分类选择、视图与时间线筛选、布局切换、已读收藏计数、
`markAllRead` 范围语义、分页游标与失败路径、搜索竞态、播放器 seek 夹取、
设置合并校验、AI per-id 流式写入等——这些正是拆分 `store.ts` 时最容易被弄坏的地方。

## 口径与行尾纪律（沿用 TASK-044/045/047 的教训）

- 行数**不用** PowerShell `Measure-Object -Line`（会少算）。
- 文本文件必须 **LF**（`.gitattributes` 为 `* text=auto eol=lf`）；
  用 Python 写文件时须显式指定行尾或用二进制模式，避免 Windows 文本模式静默写成 CRLF
  （TASK-045 曾因此被判 FAIL，且三门禁对该缺陷完全不敏感）。
- 断言计数与「既有 26 项」的区分必须在测试输出里显式给出，避免「新增断言」被误当成既有保护的增强。
