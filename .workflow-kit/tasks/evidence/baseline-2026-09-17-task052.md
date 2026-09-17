# TASK-052 基线（2026-09-17）：分页口径重设计前

采集于 TASK-051 验收之后（提交 `chore: accept TASK-051`），工作区干净。

## 授权

owner 2026-09-17 就 TASK-051 报告列出的未修项逐项裁决：P1-14 口径半边选择**『另立任务重设计』**。
本批次另授权：**可动到既有『selectFeed 不 reload』契约断言**（仅为本任务所需，且须逐条说明）。

## 问题（TASK-051 已确认并明确留下）

`loadMoreArticles` 请求下一页时不带当前范围筛选（`feed_id` / `folder_id` / `only_unread` / `only_starred`），
因此 `articlesLimit` 实际是**全局查询的 offset**。后果两条：

1. **单源/单分类视图下按全局序列翻页**——在某个源视图里下滚，取回的不是该源的后续文章；
2. **列表为空时滚动哨兵不渲染**（`Timeline` 仅在 `items.length > 0` 时渲染哨兵）
   → **该源的老文章永远够不到**。

TASK-051 已修其「失败静默吞错」半边（D2），但**未动口径**——因为那需要 per-scope 游标重设计，
且会动到既有契约断言。owner 已裁决另立任务处理。

## 为什么这不是「顺手能改的小事」

游标 `articlesLimit` 目前是**单一全局值**；改成 per-scope 意味着：

- 需要一个「范围 → 游标」的映射（范围键至少含 `feed_id` / `folder_id` / 视图筛选组合）；
- 切换范围时是**重置该范围游标**还是**触发按范围拉取**，会与既有契约
  「`selectFeed` 不 reload」产生张力——这正是 owner 授权可更新该组断言的原因；
- 空列表哨兵问题可能需要改 `Timeline` 的渲染条件（或在空态提供显式加载入口），
  这属组件层，而现有 harness **不含 DOM**（`tsconfig.test.json` 不含 `.tsx`），
  故组件侧改动只能以 `tsc` + `oxlint` + 代码审查为证——**须在报告中如实披露**。

## 基线结果（本会话实跑）

```
npm run lint          => 0 warnings and 0 errors（53 files）
npm run build         => ✓ built（tsc -b + vite）
npm run test:frontend => 186/186（既有 26 + 新增 160），退出码 0
cargo test            => exit 0
```

## 与本任务直接相关的既有断言组

- **(g) 组**（分页）：游标推进、不足一页置 `exhausted`、在途/到底防抖、失败复位 `articlesLoading`、
  游标竞态丢弃（`articlesLimit !== offset` 即丢弃）。**这些是本次改造的主要回归网**。
- **(b) 组**（范围与派生）：`selectFeed` 的范围过滤与派生树计数；
  其中的「不 reload」契约**被本任务授权更新**。

## 必须保持不变（TASK-051 刚修好，勿回退）

- **D2**：分页失败必须给出 toast + 重试 action（不得回退成静默吞错）；
- **D3**：竞态丢弃分支必须复位 `articlesLoading`（不得回退成永久为真）。
  本任务改造分页时**极易碰坏这两点**，须有断言守住。

## 口径与行尾纪律

- 行数用可信口径（LF / `splitlines()` / `ReadAllLines()`）；**不用** PowerShell `Measure-Object -Line`。
- 文本必须 **LF**；用 Python 写文件须显式指定行尾或走二进制模式。
- **台账改动必须在 `begin` 之前完成**（TASK-043 记录过、TASK-049 又犯过；
  `items/*.json` 属 `protected_paths`，begin 之后再改会被判越界，
  而 `scope` 失败**没有任何官方恢复路径**）。
- `npm` 门禁的 `args` 必须写成 `["run", "<script>"]`——我在 TASK-049 与 TASK-051 **两次**
  漏写 `run` 导致门禁执行 `npm <script>` 并报 `Unknown command`。立卡后请用脚本遍历确认。
