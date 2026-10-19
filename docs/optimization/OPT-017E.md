# OPT-017E：重试入口持久可达与有界错误提示

Status: verified-ci
父卡：OPT-017。

## 主控读码确认的事实（2026-10-16）

- `src/store/slices/ui.ts::showToast`：`toasts: [...s.toasts, {...}].slice(-4)`
  ——超过 4 条时**静默丢弃最旧的一条**；带 `action` 的重试项 4200ms 后也会被
  定时移除。
- 因此后台连续失败等场景下，唯一的重试入口可能被挤掉或超时消失，用户只能等待
  下一次自然触发。父卡明确要求「toast 上限不能悄悄挤掉唯一重试入口」。

## 交付

1. **重试可达**：错误中心（有界列表 + 去重键）或等效方案，使带重试动作的错误
   在合理时限内持久可达；不得用「无限数组暂存回调」规避上限（父卡明确禁止）。
2. **有界内存**：条目数/生命周期有明确上限，去重策略（同 key 合并最新）有测试。
3. **交互一致**：普通提示仍是现有 toast 形态与时长；错误中心入口在 UI 可见且
   可关闭；重试成功后对应条目清除。
4. **文档**：归属 Note 记录上限、去重键与取舍。

## 写集

`src/store/slices/ui.ts`、必要时 `src/store.ts`/`types.ts`、提示相关组件
（`src/components/*`）、`src/styles/base.css`、`tools/frontend-tests/*`、
`tools/component-tests/*`、归属 Note。

## 强制反例（先红后绿）

1. 连续 10 条带重试错误：全部可反思追溯（错误中心内），条数受上限约束；
   先写「唯一入口被挤掉」的失败断言。
2. 同 key 重复错误合并且保留最新重试动作；不同 key 不互相覆盖。
3. 重试成功 → 对应错误条目消失；失败 → 条目保留（不丢失入口）。
4. 普通 toast 行为不回归（时长、退场、无动作提示）。

## 门禁

`npm run lint`、`npm run build`、`npm run test:frontend`、`npm run test:components`；
Rust 未改动不强制 cargo 全量。

## 主控裁量与验收记录（2026-10-16）

- 编码会话 `code-opt017e-20261016`；独立只读复审 `audit-opt017e-20261016` 结论
  **PASS WITH FINDINGS**（P0×0 / P1×0 / P2×1 / P3 若干），复审报告
  `tmp/optimization-20261008/OPT-017E/REVIEW.md`。
- 实现：带动作 toast **双写**有界错误中心（`errors` 上限 20、TTL 24h 惰性清理、
  纯函数 `upsertErrorEntry`/`pruneErrorEntries` 为唯一实现）；普通 toast 的
  `slice(-4)` 上限、时长（2200/4200ms）与退场逐字未变。重试语义收口到单点
  `retryError`：消费先于执行（成功→条目清除、失败→同步重报保留、同步抛错→恢复快照）。
  UI：侧栏入口（非空才渲染 + 徽标计数）+ `ErrorCenterDialog`（逐条重试/忽略、
  全部清除、✕/Esc/遮罩关闭），已登记进浮层让路清单（9→10）与 App Esc 链。
- 复审动作项已收口（返工轮）：**P2 去重键=文案**触到卡内「连续 10 条错误全部可追溯」
  要求——同文案不同实体（如多个源同时刷新失败）会被合并、只能重试最新一个动作。
  改为**显式操作实体 key 优先、缺省回落文案**，20 处带重试动作调用点全部迁移
  （`feed:refresh:<id>`、`ai:summary|translate:<id>`、`reader:fulltext|content:<id>`、
  `list:load-more`、`mark:all-read`、`sync:manual`、`sync:save-and-sync` 等）；
  新反例覆盖「10 条同文案不同实体各自可追溯、逐条可重试、互不清除」+ 迁移完整性
  源级守卫（漏 key / 总数≠20 / 族前缀缺失判红）。P3 口径修正（M8 证据位于
  `red-domain.log`、App.tsx 增量行数）并如实登记残余边界。
- 不回归：无动作 toast 不进错误中心；普通 toast 时长/上限/退场逐字未变；
  `timeline-and-playback.mjs` 的浮层清单改动为**加强**（9→10 + errorCenter 探针 +
  App 实参断言），既有 9 项探针一条未删。
- 门禁 4 项 exit 全 0：`npm run lint`、`npm run build`、`npm run test:frontend`
  （1178/1178，error-center 38/38）、`npm run test:components`（44/44）。变异 M1-M10
  含 `diff -q` 逐字还原校验。
- 写集外最小增量（主控追认）：`src/App.tsx`（+12/−3，含 ToastPill 4 处接线）、
  两个 regression runner 注册、`docs/roadmap.md` 1 行、两个 harness 隔离增行。
- **事故记录**：产品提交 `fbae454` 遗漏未跟踪文件 `src/components/ErrorCenter.tsx`，
  导致 CI run 228 frontend 在 tsc 阶段报 `App.tsx:9 Cannot find module
  './components/ErrorCenter'`。补提交 `84be116`（仅补入该文件，无内容改动）后
  CI run 229 通过。教训：提交前须把 `git status` 的全部未跟踪文件与暂存清单逐项对齐。
- 遗留（不以此卡关闭）：错误中心合并粒度为操作实体（同一实体不同错误仍合并为一条）；
  TTL 为惰性清理（24h 内无新错误时超期条目仍显示，主保障是 20 条上限）；
  实机排版/窄窗/20 行滚动观感未测；重试的防双击/在途去重未做。
- 2026-10-16：提交 `84be116` 推送 dev；提交级 CI run 229
  （[38886714784](https://github.com/Dchean/fluxreader/actions/runs/38886714784)）
  completed success（8m19s）。OPT-017E 状态：verified-ci。