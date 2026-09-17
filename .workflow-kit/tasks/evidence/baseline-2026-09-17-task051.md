# TASK-051 基线（2026-09-17）：修复全部已发现缺陷前

采集于 TASK-050 验收之后（提交 `6364aed`），工作区干净。

## 授权

owner 2026-09-17：「**把发现的问题都修复**，如需越权的操作和之前一样向我申请并记录」
（`DEC-fix-all-findings-20260917`）。叠加此前的范围授权 `DEC-a2c6584f8d4442daa3a65eb0bcc7ddaf`。

**边界**：不动工作流（用户明确「工作流我另外修复」）、不改同步协议行为、不引入新依赖、
不改写 TASK-048 建立的 141 项回归网的既有语义（可新增断言、可更新那两条把现状写成期望的
「观察项」断言，但须逐条说明）。

## 待修清单来源（本任务第一步是**重新盘点**）

工作流要求「清单经确认后才纳入处理」；owner 已就「全部修复」预授权，
故盘点结果在本任务报告里给出并逐项落实，不再单独等待确认。

### A. TASK-048 写测试时发现的 store 缺陷（5 项，**当时按纪律未修**）

拆分后（TASK-049）的位置由审查者按行核对过：

| # | 缺陷 | 拆分前位置 | 拆分后位置 |
| --- | --- | --- | --- |
| D1a | AI toast「重试」是**死按钮**：摘要流先产出半截文本再报错时，重试撞上 `if (art.aiSummary) return;` → 不重发 IPC、`summaryErrors` 不清 | `store.ts:762-764` | `slices/ai.ts:192` |
| D1b | 同源：翻译的 `if (!art || art.translatedContent) return;` | `store.ts:515` | `slices/ai.ts:39` |
| D2 | `loadMoreArticles` 失败路径 `catch { set({ articlesLoading:false }); }` **完全吞错**（无 toast、无错误态） | `store.ts:1030-1032` | `slices/bootstrap.ts:150-151` |
| D3 | 竞态丢弃分支 `if (get().articlesLimit !== offset) return;` **不复位 `articlesLoading`** → 会被入口守卫永久挡住后续加载 | `store.ts:1012-1013` | `slices/bootstrap.ts:127`(守卫) + `:133`(丢弃) |
| D4 | 「启动时打开」白名单 `all\|today\|unread\|starred` 与设置页下拉 `unread\|all\|today\|article` **不同步** → 选「文章」落库但启动时静默失效 | `store.ts:1581` | `slices/settings.ts:75` |
| D5 | `updateSettings` **无运行时校验**：探针 `fontSize:-5 / refreshInterval:0 / fetchConcurrency:99 / listWidth:99999` 全部落库 | `store.ts:1551-1557` | `slices/settings.ts:45` |

**D2 ≡ REQ-007 的 P1-14；D4 ≡ REQ-007 的 P1-15。**

### B. TASK-048 报告记录的低优先观察（2 项）

| # | 观察 | 位置 |
| --- | --- | --- |
| L1 | `markEntriesReadBulk` 逐 id `entries.find` → O(n·m)，与 `markEntriesRead` 的一次遍历优化不一致 | 拆分前 `store.ts:732-735` |
| L2 | `selectViewCounts` / `selectTreeCounts` 对 `feedCounts` 缺失的源直接 `continue`（角标空且不计入「全部」数字，但条目仍列出） | `src/store/selectors.ts:131-132、167-168` |

### C. REQ-007 残余项（**须重新盘点**，不得沿用旧结论）

`FINDINGS-REQ-007.md` 共 45 条，其中 P0/P1 多数已由 TASK-030~043 处理。
本会话此前实测**仍存在**的有：P1-14（≡D2）、P1-15（≡D4）、P2-4（「保存提示词」实际保存整套端点配置）、
P2-7（`compareVersions(remote, '')` 恒大于 0）。

**但第一步必须逐项重新验证**并明确列出「经复核已不存在」的项——
避免把已修好的当未修（这正是工作流对 REQ-007 的要求）。

### D. 审查者指出的两个边界（并入本次）

| # | 内容 |
| --- | --- |
| E1 | `bootstrapGithubAuth` 是 store 拆分中**唯一**从模块级函数改为「slice 导出 + 晚绑定句柄」的迁移点，而 141 项断言**从未调用它** → 补断言，或登记为已知不可测项并写明理由 |
| E2 | `src/store/internals.ts` 的 `appStore()` 是未校验的 `as` 断言（当前不可达）→ 改显式 `throw` |

## 基线结果（本会话实跑）

```
npm run lint          => 0 warnings and 0 errors（49 files）
npm run build         => ✓ built（tsc -b + vite）
npm run test:frontend => 141/141（既有 26 + TASK-048 新增 115），退出码 0，连跑两次一致
cargo test            => 120 passed / 0 failed / 23 ignored
```

**本任务行为会变**（这正是目的），故回归基线不是「保持不变」，
而是「**既有保护不削弱 + 每处行为变化都有对应断言更新与理由**」。

## 需要特别注意的两条「把现状写成期望」的断言

TASK-048 的独立审查者预先标注：有两条断言把 D2/D5 邻域的现状写成了期望
（断言名自带「观察项」）。**修好 D2/D5 后它们必然失败**，本任务必须同步更新，
并逐条说明改动理由——否则「修好缺陷反而让测试失败」。

## 口径与行尾纪律

- 行数用可信口径（LF 字节数 / `splitlines()` / `ReadAllLines()`）；**不用** PowerShell `Measure-Object -Line`。
- 文本必须 **LF**；用 Python 写文件须显式指定行尾或走二进制模式。
- 台账改动必须在 `begin` **之前**完成（TASK-043 记录过、TASK-049 又犯过）。
- 派生值（如审查摘要）必须用**产生它的那个函数**重算，不要自建副本重实现规则
  （我在 TASK-049 的工作流修复中因此写出过错值）。
