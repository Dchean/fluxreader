# OPT-017E：重试入口持久可达与有界错误提示

Status: queued
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
